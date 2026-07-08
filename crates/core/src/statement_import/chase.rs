//! Chase-specific statement text parsing (spec §6.3, D28) — the
//! reference `StatementParser` implementation. Vanguard/Morgan
//! Stanley/Fidelity follow later as independent, sibling parsers behind
//! the same trait (see `parser.rs`).
//!
//! Parses already-extracted plain text (from `pdf_text::extract_text`),
//! never a PDF file itself — kept a pure string-in/struct-out function
//! so it's testable against inline fixtures without any PDF I/O.
//!
//! Handles two distinct real Chase layouts (checking/savings and credit
//! card), both confirmed against real statement structure — field
//! labels and section headers only, no real balances/account numbers/
//! names ever appear in this file or its tests:
//! - **Checking/savings**: `"Account ending in ####"` (straight digits,
//!   no masking) paired with the nearest following `"Balance $X"`.
//! - **Credit card** (e.g. Sapphire Reserve): `"Account Number:  XXXX
//!   XXXX XXXX ####"` (masked groups, last 4 real) paired with `"New
//!   Balance $X"` — `"Balance $"` is a substring of `"New Balance $"`,
//!   so the same balance-marker search already covers both. Category
//!   detection (`detect_category`) also uses this layout's real
//!   liability terminology: `"Credit Access Line"` (this card's actual
//!   wording — notably *not* the generic `"Credit Limit"` originally
//!   guessed) and `"Available Credit"`/`"Minimum Payment Due"`, both
//!   confirmed present verbatim.
//!
//! Both account-id markers are found via the same
//! "skip past any non-digit masking characters, then capture the digit
//! run" helper — it happens to work for both layouts unchanged, since
//! `"Account ending in "` has zero masking characters to skip and
//! `"Account Number:  XXXX XXXX XXXX "` has several.

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
/// ####"` or credit card's `"Account Number:"`) and the `"Balance $X"`
/// that follows it. Searches loosely by substring position rather than
/// assuming a fixed line structure, since `pdf-extract`'s line breaks
/// don't necessarily match the statement's original visual layout.
fn extract_account_sections(text: &str) -> Vec<AccountSection> {
    const MARKERS: &[&str] = &["Account ending in ", "Account Number:"];
    let mut sections = Vec::new();

    for marker in MARKERS {
        let mut search_from = 0;
        while let Some(rel_pos) = text[search_from..].find(marker) {
            let after_marker = search_from + rel_pos + marker.len();
            if let Some(last4) = extract_last4(&text[after_marker..]) {
                if let Some(balance) = find_next_balance(&text[after_marker..]) {
                    sections.push(AccountSection { last4, balance });
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
/// the first 4-digit run found. Bounded so a marker with no digits
/// anywhere nearby (e.g. right at the end of the document) can't walk
/// off into unrelated later text.
fn extract_last4(text: &str) -> Option<String> {
    let window: String = text.chars().take(40).collect();
    let digit_start = window.find(|c: char| c.is_ascii_digit())?;
    let last4: String = window[digit_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    (last4.len() == 4).then_some(last4)
}

/// Finds the next `"Balance $X"` after the given position and parses
/// `X` as a decimal amount, stripping thousands-separator commas.
fn find_next_balance(text: &str) -> Option<f64> {
    const MARKER: &str = "Balance $";
    let amount_start = text.find(MARKER)? + MARKER.len();
    let amount: String = text[amount_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    amount.replace(',', "").parse().ok()
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
}
