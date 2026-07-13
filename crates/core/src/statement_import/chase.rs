//! Chase-specific statement text parsing (spec §6.3, D28) — the
//! reference `StatementParser` implementation. Vanguard/Morgan
//! Stanley/Fidelity follow later as independent, sibling parsers behind
//! the same trait (see `parser.rs`).
//!
//! Parses already-extracted plain text (from `pdf_text::extract_text`),
//! never a PDF file itself — kept a pure string-in/struct-out function
//! so it's testable against inline fixtures without any PDF I/O.
//!
//! Handles three distinct real Chase layouts (two checking/savings
//! variants and credit card), all confirmed against real statement
//! structure — field labels and section headers only, no real
//! balances/account numbers/names ever appear in this file or its
//! tests:
//! - **Checking/savings** (one real statement): `"Account ending in
//!   ####"` (straight digits, no masking) paired with the nearest
//!   following `"Balance $X"`.
//! - **Credit card** (e.g. Sapphire Reserve): `"Account Number:  XXXX
//!   XXXX XXXX ####"` (masked groups, last 4 real) paired with `"New
//!   Balance $X"` — `"Balance $"` is a substring of `"New Balance $"`,
//!   so the same balance-marker search already covers both. Category
//!   detection (`detect_category`) also uses this layout's real
//!   liability terminology: `"Credit Access Line"` (this card's actual
//!   wording — notably *not* the generic `"Credit Limit"` originally
//!   guessed) and `"Available Credit"`/`"Minimum Payment Due"`, both
//!   confirmed present verbatim.
//! - **Checking/savings, a second real statement** (Chase Total
//!   Checking, spec D34): also uses `"Account Number:"`, but followed
//!   by the **full, unmasked** account number rather than a masked
//!   last-4 group — the account-id extraction takes the last 4 digits
//!   of whatever digit run it finds, which happens to already be
//!   correct for the masked case too (a 4-digit run's "last 4" is
//!   itself). Its `"CHECKING SUMMARY"` table also splits each row's
//!   label and dollar value onto separate lines (a pdf-extract
//!   column-order artifact, same category as Vanguard's fund-table
//!   quirk) — `"Ending Balance"` isn't immediately followed by `"$"`,
//!   so balance extraction falls back to a bounded forward search past
//!   the label for the next `"$"` when the adjacent-substring search
//!   finds nothing. **This statement also repeats `"Account Number:"`
//!   on every page** — a real, previously-unexercised case that a
//!   single-page synthetic fixture never caught: without deduping by
//!   last4, each repeat was read as a distinct account and rejected as
//!   `AmbiguousMatch` even though it's the same one account throughout.
//!
//! Both account-id markers are found via the same
//! "skip past any non-digit masking characters, then capture a digit
//! run, keep its last 4" helper — it happens to work for all three
//! layouts unchanged, since `"Account ending in "` and the masked
//! credit-card group both yield an already-4-digit run, and the full
//! unmasked number yields a longer one whose last 4 digits are taken.

use std::collections::HashSet;

use crate::statement_import::parser::{ExpectedAccount, ParseError, ParsedStatement, StatementParser};
use crate::Category;

pub struct ChaseStatementParser;

impl StatementParser for ChaseStatementParser {
    fn institution(&self) -> &'static str {
        "chase"
    }

    fn parse(
        &self,
        text: &str,
        expected: &ExpectedAccount,
    ) -> Result<ParsedStatement, ParseError> {
        let accounts = extract_account_sections(text);
        if accounts.is_empty() {
            return Err(ParseError::UnrecognizedLayout(
                "no 'Account ending in ####' or 'Account Number:' section found".into(),
            ));
        }

        let matches: Vec<&AccountSection> = accounts
            .iter()
            .filter(|a| match &expected.account_hint {
                Some(hint) => &a.last4 == hint,
                None => true,
            })
            .collect();

        match matches.len() {
            0 => Err(ParseError::NoMatchingAccount),
            1 => {
                let account = matches[0];
                Ok(ParsedStatement {
                    balance: account.balance,
                    as_of_date: extract_statement_date(text).unwrap_or_default(),
                    account_identifier: account.last4.clone(),
                    category: detect_category(text),
                    holdings: vec![],
                    apr: extract_purchases_apr(text),
                })
            }
            _ => Err(ParseError::AmbiguousMatch),
        }
    }
}

struct AccountSection {
    last4: String,
    balance: f64,
}

/// Finds every account-id marker (checking/savings' `"Account ending in
/// ####"` or `"Account Number:"`, either masked or fully unmasked) and
/// the balance that follows it. Searches loosely by substring position
/// rather than assuming a fixed line structure, since `pdf-extract`'s
/// line breaks don't necessarily match the statement's original visual
/// layout. **Deduped by last4** (spec D34) — a real multi-page
/// statement repeats its account-id marker on every page, which would
/// otherwise be misread as multiple distinct accounts and rejected as
/// `AmbiguousMatch` even though every occurrence names the same one
/// account. The first occurrence whose balance can be found wins;
/// later repeats of the same last4 are skipped outright.
fn extract_account_sections(text: &str) -> Vec<AccountSection> {
    const MARKERS: &[&str] = &["Account ending in ", "Account Number:"];
    let mut sections = Vec::new();
    let mut seen_last4s: HashSet<String> = HashSet::new();

    for marker in MARKERS {
        let mut search_from = 0;
        while let Some(rel_pos) = text[search_from..].find(marker) {
            let after_marker = search_from + rel_pos + marker.len();
            if let Some(last4) = extract_last4(&text[after_marker..]) {
                if !seen_last4s.contains(&last4) {
                    if let Some(balance) = find_next_balance(&text[after_marker..]) {
                        seen_last4s.insert(last4.clone());
                        sections.push(AccountSection { last4, balance });
                    }
                }
            }

            search_from = after_marker;
            if search_from >= text.len() {
                break;
            }
        }
    }

    sections
}

/// Skips past up to 40 non-digit characters (masking `X`s, spaces —
/// `"Account Number:  XXXX XXXX XXXX ####"`'s masked groups, or nothing
/// at all for `"Account ending in ####"`, which has none) and captures
/// the digit run found there, keeping only its last 4 digits. A masked
/// group or `"Account ending in"` already yields a 4-digit run, whose
/// "last 4" is itself; a real Chase Total Checking statement's
/// `"Account Number:"` instead shows the **full, unmasked** account
/// number (spec D34) — a much longer digit run — so this always takes
/// the last 4 rather than requiring the run to already be exactly 4.
/// Bounded so a marker with no digits anywhere nearby (e.g. right at
/// the end of the document) can't walk off into unrelated later text.
fn extract_last4(text: &str) -> Option<String> {
    let window: String = text.chars().take(40).collect();
    let digit_start = window.find(|c: char| c.is_ascii_digit())?;
    let digits: String = window[digit_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (digits.len() >= 4).then(|| digits[digits.len() - 4..].to_string())
}

/// Finds the next balance after the given position and parses it as a
/// decimal amount, stripping thousands-separator commas. Tries the
/// adjacent `"Balance $X"` substring first (credit card's `"New Balance
/// $X"`, and the checking/savings layout confirmed against an earlier
/// real statement); falls back to `"Ending Balance"` followed — a few
/// lines later, not adjacent — by the next `"$"` (a real Chase Total
/// Checking statement's `"CHECKING SUMMARY"` table, spec D34, which
/// splits each row's label and dollar value onto separate lines).
fn find_next_balance(text: &str) -> Option<f64> {
    find_amount_adjacent_to(text, "Balance $").or_else(|| find_amount_after_label(text, "Ending Balance", 40))
}

fn find_amount_adjacent_to(text: &str, marker: &str) -> Option<f64> {
    let amount_start = text.find(marker)? + marker.len();
    parse_amount(&text[amount_start..])
}

/// Finds `label`, then the next `"$"` within `window` characters after
/// it (tolerating blank lines/other short lines pdf-extract inserted
/// between a table's label and value column), and parses the amount
/// that follows.
fn find_amount_after_label(text: &str, label: &str, window: usize) -> Option<f64> {
    let start = text.find(label)? + label.len();
    let end = (start + window).min(text.len());
    let slice = &text[start..end];
    let dollar_pos = slice.find('$')?;
    parse_amount(&slice[dollar_pos + 1..])
}

fn parse_amount(text: &str) -> Option<f64> {
    let amount: String = text
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    (!amount.is_empty()).then(|| amount.replace(',', "")).and_then(|a| a.parse().ok())
}

/// Best-effort statement date, matching `"Statement Date: MM/DD/YYYY"`.
/// Returns `None` (not an error) if absent — a missing date shouldn't
/// block extracting a balance that's otherwise valid.
fn extract_statement_date(text: &str) -> Option<String> {
    const MARKER: &str = "Statement Date: ";
    let start = text.find(MARKER)? + MARKER.len();
    let raw: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '/')
        .collect();
    (!raw.is_empty()).then_some(raw)
}

/// The credit-card layout's interest rate (spec D42) — verified against
/// a real Chase Sapphire Reserve statement's `"INTEREST CHARGES"`
/// section, which lists a *separate* rate per balance type (`PURCHASES`,
/// `CASH ADVANCES`, `BALANCE TRANSFERS / MY CHASE LOAN`), each formatted
/// like `"19.49%(v)(d)"`. Only the `PURCHASES` row's rate is extracted —
/// that's the rate that applies to an ordinary carried/revolving
/// balance; cash-advance and balance-transfer rates are edge cases, and
/// "My Chase Loan" is a separate installment product, not a revolving
/// balance. Finding `"PURCHASES"` *within* the `"INTEREST CHARGES"`
/// section first (rather than just taking the first `%` after `"INTEREST
/// CHARGES"`) is what correctly skips past `"CASH ADVANCES"`' rate,
/// which appears later in the same real table. Checking/savings
/// statements have no such section at all, so this returns `None` for
/// them — a missing rate never blocks a valid balance, same as every
/// other optional field in this module.
fn extract_purchases_apr(text: &str) -> Option<f64> {
    const SECTION_MARKER: &str = "INTEREST CHARGES";
    let section_start = text.find(SECTION_MARKER)?;
    extract_percentage_after(&text[section_start..], "PURCHASES", 300)
}

/// Finds `marker`, then the first ASCII digit within `window` characters
/// after it, and parses the digit/decimal-point run starting there as a
/// percentage. Works whether the digits are immediately followed by `%`
/// (`"19.49%"`) or not — the run itself never includes `%` or
/// whitespace either way.
fn extract_percentage_after(text: &str, marker: &str, window: usize) -> Option<f64> {
    let start = text.find(marker)? + marker.len();
    let end = (start + window).min(text.len());
    let slice = &text[start..end];
    let digit_start = slice.find(|c: char| c.is_ascii_digit())?;
    let number: String = slice[digit_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    number.parse().ok()
}

/// Chase is the only institution this app supports (spec FR1) with both
/// asset (checking/savings) and liability (credit card) statements, so
/// unlike Vanguard/Fidelity — which never need this at all — a Chase
/// statement's category can't be assumed from the institution alone.
///
/// Verified against a real Chase Sapphire Reserve statement (field
/// labels only, never a real balance/account number/name): it says
/// `"Minimum Payment Due"` and `"Available Credit"` verbatim, but
/// **not** `"Credit Limit"` — it uses `"Credit Access Line"` instead.
/// The original heuristic (guessed before any real credit-card
/// statement had been seen) would have missed this real statement
/// entirely; kept `"Credit Limit"` alongside it since other Chase card
/// products may still use that wording.
fn detect_category(text: &str) -> Category {
    const LIABILITY_MARKERS: &[&str] = &[
        "Minimum Payment Due",
        "Credit Limit",
        "Credit Access Line",
        "Available Credit",
    ];
    let lower = text.to_lowercase();
    let is_liability = LIABILITY_MARKERS
        .iter()
        .any(|marker| lower.contains(&marker.to_lowercase()));
    if is_liability {
        Category::Liability
    } else {
        Category::Asset
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected(account_hint: Option<&str>) -> ExpectedAccount {
        ExpectedAccount {
            account_type: "checking".into(),
            account_hint: account_hint.map(String::from),
        }
    }

    #[test]
    fn parses_a_single_matching_account() {
        let text = "CHASE\nChase Checking Statement\nAccount ending in 6789\n\
                     Statement Date: 06/30/2026\nNew Balance $1,234.56\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.balance, 1234.56);
        assert_eq!(result.as_of_date, "06/30/2026");
        assert_eq!(result.account_identifier, "6789");
    }

    #[test]
    fn disambiguates_multiple_accounts_via_last4() {
        let text = "Statement Date: 06/30/2026\n\
                     Account ending in 1111\nNew Balance $500.00\n\
                     Account ending in 6789\nNew Balance $1,234.56\n";

        let result = ChaseStatementParser
            .parse(text, &expected(Some("6789")))
            .unwrap();

        assert_eq!(result.balance, 1234.56);
        assert_eq!(result.account_identifier, "6789");
    }

    #[test]
    fn errors_when_no_account_matches_the_given_last4() {
        let text = "Account ending in 1111\nNew Balance $500.00\n";

        let err = ChaseStatementParser
            .parse(text, &expected(Some("9999")))
            .unwrap_err();

        assert_eq!(err, ParseError::NoMatchingAccount);
    }

    #[test]
    fn errors_when_multiple_accounts_match_and_no_last4_was_given() {
        let text = "Account ending in 1111\nNew Balance $500.00\n\
                     Account ending in 6789\nNew Balance $1,234.56\n";

        let err = ChaseStatementParser.parse(text, &expected(None)).unwrap_err();

        assert_eq!(err, ParseError::AmbiguousMatch);
    }

    #[test]
    fn errors_on_text_with_no_recognizable_chase_layout() {
        let text = "this text does not look like a Chase statement at all";

        let err = ChaseStatementParser.parse(text, &expected(None)).unwrap_err();

        assert!(matches!(err, ParseError::UnrecognizedLayout(_)));
    }

    #[test]
    fn missing_statement_date_does_not_block_a_valid_balance() {
        let text = "Account ending in 6789\nNew Balance $1,234.56\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.balance, 1234.56);
        assert_eq!(result.as_of_date, "");
    }

    #[test]
    fn a_checking_statement_is_categorized_as_an_asset() {
        let text = "CHASE\nChase Checking Statement\nAccount ending in 6789\n\
                     Statement Date: 06/30/2026\nNew Balance $1,234.56\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.category, Category::Asset);
    }

    #[test]
    fn a_statement_with_credit_card_terminology_is_categorized_as_a_liability() {
        let text = "CHASE\nAccount ending in 6789\nNew Balance $1,234.56\n\
                     Minimum Payment Due $35.00\nCredit Limit $10,000.00\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.category, Category::Liability);
    }

    #[test]
    fn credit_card_terminology_is_matched_case_insensitively() {
        let text = "CHASE\nAccount ending in 6789\nNew Balance $1,234.56\n\
                     minimum payment due $35.00\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.category, Category::Liability);
    }

    #[test]
    fn parses_a_credit_card_statement_using_the_account_number_marker() {
        let text = "CHASE\nAccount Number:  XXXX XXXX XXXX 4321\n\
                     Statement Date: 06/30/2026\nNew Balance $567.89\n\
                     Minimum Payment Due $35.00\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.balance, 567.89);
        assert_eq!(result.as_of_date, "06/30/2026");
        assert_eq!(result.account_identifier, "4321");
        assert_eq!(result.category, Category::Liability);
    }

    #[test]
    fn credit_access_line_is_recognized_as_liability_terminology() {
        // The real wording this card uses instead of the more generic
        // "Credit Limit" guessed before any real statement was seen.
        let text = "CHASE\nAccount Number:  XXXX XXXX XXXX 4321\n\
                     New Balance $567.89\nBalance over the Credit Access Line $0.00\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.category, Category::Liability);
    }

    #[test]
    fn disambiguates_a_credit_card_account_via_account_hint() {
        let text = "Account Number:  XXXX XXXX XXXX 1111\nNew Balance $100.00\n\
                     Account Number:  XXXX XXXX XXXX 4321\nNew Balance $567.89\n";

        let result = ChaseStatementParser
            .parse(text, &expected(Some("4321")))
            .unwrap();

        assert_eq!(result.balance, 567.89);
        assert_eq!(result.account_identifier, "4321");
    }

    #[test]
    fn a_full_unmasked_account_number_is_reduced_to_its_last_4_digits() {
        // Spec D34: a real Chase Total Checking statement's "Account
        // Number:" is followed by the full account number, not a
        // masked last-4 group.
        let text = "CHASE\nAccount Number:\n\n1234567890\n\nNew Balance $250.00\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.account_identifier, "7890");
        assert_eq!(result.balance, 250.00);
    }

    #[test]
    fn ending_balance_split_across_lines_from_its_dollar_value_is_still_found() {
        // Spec D34: this statement's "CHECKING SUMMARY" table puts each
        // row's label and dollar value on separate lines/paragraphs (a
        // pdf-extract column-order artifact), so "Balance $" never
        // appears as an adjacent substring for this layout.
        let text = "CHASE\nAccount Number:\n\n1234567890\n\n\
                     CHECKING SUMMARY\n\nBeginning Balance\n\n$100.00\n\n\
                     Ending Balance\n\n$250.00\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.balance, 250.00);
    }

    #[test]
    fn the_same_account_repeated_across_pages_is_not_treated_as_ambiguous() {
        // Spec D34: a real multi-page statement repeats "Account
        // Number:" (with the same account number) on every page —
        // without deduping by last4, this was misread as multiple
        // distinct accounts and rejected as AmbiguousMatch.
        let text = "CHASE\nAccount Number:\n\n1234567890\n\n\
                     Ending Balance\n\n$250.00\n\n\
                     Account Number:\n\n1234567890\n\n\
                     Ending Balance\n\n$250.00\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.balance, 250.00);
        assert_eq!(result.account_identifier, "7890");
    }

    #[test]
    fn extracts_the_purchases_apr_not_cash_advances_or_balance_transfers() {
        // Real Chase Sapphire Reserve structure: separate rates per
        // balance type, Purchases listed before Cash Advances — proves
        // the two-step "INTEREST CHARGES" then "PURCHASES" search picks
        // the right row, not just the first "%" found in the section.
        let text = "CHASE\nAccount Number:  XXXX XXXX XXXX 4321\n\
                     New Balance $567.89\n\
                     INTEREST CHARGES\n\
                     Your Annual Percentage Rate (APR) is the annual interest rate on your account.\n\
                     PURCHASES\n  Purchases                    19.49%(v)(d)         -0-    -0-\n\
                     CASH ADVANCES\n  Cash Advances                28.49%(v)(d)         -0-    -0-\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.apr, Some(19.49));
    }

    #[test]
    fn a_checking_statement_with_no_interest_charges_section_has_no_apr() {
        let text = "CHASE\nChase Checking Statement\nAccount ending in 6789\n\
                     Statement Date: 06/30/2026\nNew Balance $1,234.56\n";

        let result = ChaseStatementParser.parse(text, &expected(None)).unwrap();

        assert_eq!(result.apr, None);
        assert_eq!(result.balance, 1234.56);
    }
}
