//! Chase-specific statement text parsing (spec §6.3, D28) — the
//! reference `StatementParser` implementation. Vanguard/Morgan
//! Stanley/Fidelity follow later as independent, sibling parsers behind
//! the same trait (see `parser.rs`).
//!
//! Parses already-extracted plain text (from `pdf_text::extract_text`),
//! never a PDF file itself — kept a pure string-in/struct-out function
//! so it's testable against inline fixtures without any PDF I/O.
//!
//! Heuristic, not a full grammar: looks for `"Account ending in ####"`
//! blocks and takes the next `"Balance $X"` after each one as that
//! account's balance. This assumes Chase pairs an account's own
//! identifying line near its own balance line (true of the real Chase
//! statement layout this was modeled on) rather than grouping all
//! account numbers separately from all balances — a real-world fixture
//! should confirm this once one is available, per the reference-only
//! caveat already noted for this synthetic fixture.

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
                "no 'Account ending in ####' section found".into(),
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

/// Finds every `"Account ending in ####"` and the `"Balance $X"` that
/// follows it. Searches loosely by substring position rather than
/// assuming a fixed line structure, since `pdf-extract`'s line breaks
/// don't necessarily match the statement's original visual layout.
fn extract_account_sections(text: &str) -> Vec<AccountSection> {
    const MARKER: &str = "Account ending in ";
    let mut sections = Vec::new();
    let mut search_from = 0;

    while let Some(rel_pos) = text[search_from..].find(MARKER) {
        let last4_start = search_from + rel_pos + MARKER.len();
        let last4: String = text[last4_start..]
            .chars()
            .take_while(|c| c.is_ascii_digit())
            .collect();

        if last4.len() == 4 {
            if let Some(balance) = find_next_balance(&text[last4_start..]) {
                sections.push(AccountSection { last4, balance });
            }
        }

        search_from = last4_start;
        if search_from >= text.len() {
            break;
        }
    }

    sections
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
/// **Unverified heuristic**: checks for generic, universal credit-card
/// statement terminology, not anything modeled on a real Chase
/// credit-card layout — only Chase's *checking* statement structure was
/// confirmed against real statement wording (see this file's own
/// module-level caveat above). A real Chase credit-card statement has
/// never been seen while building this parser.
fn detect_category(text: &str) -> Category {
    const LIABILITY_MARKERS: &[&str] =
        &["Minimum Payment Due", "Credit Limit", "Available Credit"];
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
}
