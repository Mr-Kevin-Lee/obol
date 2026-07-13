//! Apple Card (Goldman Sachs Bank USA) statement text parsing (spec
//! §6.3, D28/D30) — verified against a real Apple Card statement
//! exported as a PDF from the Wallet app (field labels only, never a
//! real balance/account number/name/email). Supersedes spec FR2's
//! original "entered manually each run" plan for this institution — a
//! downloadable PDF statement exists after all, so it fits the same
//! `StatementParser` pattern as every other institution rather than
//! needing a separate manual-entry path.
//!
//! Always a liability (a credit card has no asset variant), so unlike
//! `chase.rs` this never needs content-based category detection.
//!
//! No card/account number appears anywhere in this statement layout —
//! same situation as `fidelity.rs`'s NetBenefits statements having no
//! account number. The closest stable per-statement identifier is the
//! `"Apple Card Customer"` line, which names the cardholder — used the
//! same way Fidelity uses its plan-name line: as the raw
//! `account_identifier`, hashed by the caller, never persisted or
//! reproduced raw.
//!
//! `"Total Balance $X"` is the canonical current-balance marker — the
//! statement also shows a `"Previous Total Balance $X"` for the prior
//! period, and since `"Total Balance $"` is literally a substring of
//! `"Previous Total Balance $"`, a naive search would silently grab the
//! wrong (stale) figure. `find_current_total_balance` explicitly skips
//! any match immediately preceded by `"Previous "`.

use crate::statement_import::parser::{ExpectedAccount, ParseError, ParsedStatement, StatementParser};
use crate::Category;

pub struct AppleCardStatementParser;

impl StatementParser for AppleCardStatementParser {
    fn institution(&self) -> &'static str {
        "applecard"
    }

    fn parse(
        &self,
        text: &str,
        expected: &ExpectedAccount,
    ) -> Result<ParsedStatement, ParseError> {
        let Some((balance, after_balance)) = find_current_total_balance(text) else {
            return Err(ParseError::UnrecognizedLayout(
                "no standalone 'Total Balance $' section found".into(),
            ));
        };

        let Some(account_identifier) = extract_customer_line(text) else {
            return Err(ParseError::UnrecognizedLayout(
                "no 'Apple Card Customer' line found".into(),
            ));
        };

        if let Some(hint) = &expected.account_hint {
            if !account_identifier.to_lowercase().contains(&hint.to_lowercase()) {
                return Err(ParseError::NoMatchingAccount);
            }
        }

        Ok(ParsedStatement {
            balance,
            as_of_date: extract_as_of_date(after_balance).unwrap_or_default(),
            account_identifier,
            category: Category::Liability,
            holdings: vec![],
            apr: extract_apr(text),
        })
    }
}

/// Finds the standalone `"Total Balance $X"` marker (never the
/// `"Previous Total Balance $X"` one immediately before it in the
/// statement) and returns the parsed amount plus the text slice
/// immediately following it, so the caller can scope its own
/// `"as of "` search to the date attached to *this* balance rather than
/// an earlier `"Previous ..."` section's date.
fn find_current_total_balance(text: &str) -> Option<(f64, &str)> {
    const MARKER: &str = "Total Balance $";
    const EXCLUDE_PREFIX: &str = "Previous ";
    let mut search_from = 0;

    while let Some(rel_pos) = text[search_from..].find(MARKER) {
        let marker_pos = search_from + rel_pos;
        let amount_start = marker_pos + MARKER.len();

        if !text[..marker_pos].ends_with(EXCLUDE_PREFIX) {
            let amount: String = text[amount_start..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
                .collect();
            let balance = amount.replace(',', "").parse().ok()?;
            return Some((balance, &text[amount_start..]));
        }

        search_from = amount_start;
        if search_from >= text.len() {
            break;
        }
    }

    None
}

/// Apple Card's interest rate (spec D42) — a single flat rate, unlike
/// Chase's per-balance-type table: `"Annual Percentage Rate (APR) 14.49
/// % (variable)"`. Note the space before `%`, unlike Chase's adjacent
/// `"19.49%"` — the digit/decimal-point run stops at the space either
/// way, so no separate handling is needed. Returns `None` (not an
/// error) if the marker is absent, same missing-field convention as
/// every other optional value in this module.
fn extract_apr(text: &str) -> Option<f64> {
    const MARKER: &str = "Annual Percentage Rate (APR)";
    let start = text.find(MARKER)? + MARKER.len();
    let window = &text[start..(start + 40).min(text.len())];
    let digit_start = window.find(|c: char| c.is_ascii_digit())?;
    let number: String = window[digit_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '.')
        .collect();
    number.parse().ok()
}

/// Best-effort as-of date, matching `"as of Month DD, YYYY"`. Returns
/// `None` (not an error) if absent, same missing-date-doesn't-block-a-
/// balance rule as every other parser in this module.
fn extract_as_of_date(text: &str) -> Option<String> {
    const MARKER: &str = "as of ";
    let start = text.find(MARKER)? + MARKER.len();
    let raw: String = text[start..]
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == ' ' || *c == ',')
        .collect();
    let trimmed = raw.trim_end_matches(' ').to_string();
    (!trimmed.is_empty()).then_some(trimmed)
}

/// The line immediately after `"Apple Card Customer"` — a running
/// header repeated on every page, so only the first occurrence is
/// needed. Whatever text is there (cardholder name/email) is treated
/// as an opaque raw identifier, hashed by the caller, never inspected
/// or reproduced by this parser itself.
fn extract_customer_line(text: &str) -> Option<String> {
    const MARKER: &str = "Apple Card Customer";
    let start = text.find(MARKER)? + MARKER.len();
    let after_marker = text[start..].trim_start_matches(['\n', '\r']);
    let line_end = after_marker.find('\n').unwrap_or(after_marker.len());
    let line = after_marker[..line_end].trim();
    (!line.is_empty()).then(|| line.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected(account_hint: Option<&str>) -> ExpectedAccount {
        ExpectedAccount {
            account_type: "credit_card".into(),
            account_hint: account_hint.map(String::from),
        }
    }

    #[test]
    fn parses_the_current_total_balance_not_the_previous_one() {
        let text = "Apple Card Customer\nJane Doe, jane@example.com Statement\n\
                     Account Information\n\
                     Previous Monthly Balance $200.00 \nas of Mar 31, 2026\n\n\
                     Previous Total Balance $200.00 \nas of Mar 31, 2026\n\n\
                     Total Balance $567.89 \nas of Apr 30, 2026\n";

        let result = AppleCardStatementParser
            .parse(text, &expected(None))
            .unwrap();

        assert_eq!(result.balance, 567.89);
        assert_eq!(result.as_of_date, "Apr 30, 2026");
        assert_eq!(result.account_identifier, "Jane Doe, jane@example.com Statement");
        assert_eq!(result.category, Category::Liability);
    }

    #[test]
    fn always_categorizes_as_a_liability() {
        let text = "Apple Card Customer\nJane Doe Statement\n\
                     Total Balance $10.00 \nas of Apr 30, 2026\n";

        let result = AppleCardStatementParser
            .parse(text, &expected(None))
            .unwrap();

        assert_eq!(result.category, Category::Liability);
    }

    #[test]
    fn account_hint_matches_a_substring_of_the_customer_line() {
        let text = "Apple Card Customer\nJane Doe Statement\n\
                     Total Balance $10.00 \nas of Apr 30, 2026\n";

        let result = AppleCardStatementParser
            .parse(text, &expected(Some("jane")))
            .unwrap();

        assert_eq!(result.balance, 10.00);
    }

    #[test]
    fn errors_when_the_account_hint_does_not_match() {
        let text = "Apple Card Customer\nJane Doe Statement\n\
                     Total Balance $10.00 \nas of Apr 30, 2026\n";

        let err = AppleCardStatementParser
            .parse(text, &expected(Some("nonexistent name")))
            .unwrap_err();

        assert_eq!(err, ParseError::NoMatchingAccount);
    }

    #[test]
    fn errors_on_text_with_no_recognizable_apple_card_layout() {
        let text = "this text does not look like an Apple Card statement at all";

        let err = AppleCardStatementParser
            .parse(text, &expected(None))
            .unwrap_err();

        assert!(matches!(err, ParseError::UnrecognizedLayout(_)));
    }

    #[test]
    fn missing_as_of_date_does_not_block_a_valid_balance() {
        let text = "Apple Card Customer\nJane Doe Statement\nTotal Balance $10.00\n";

        let result = AppleCardStatementParser
            .parse(text, &expected(None))
            .unwrap();

        assert_eq!(result.balance, 10.00);
        assert_eq!(result.as_of_date, "");
    }

    #[test]
    fn extracts_the_annual_percentage_rate() {
        // Real Apple Card structure: a single flat rate with a space
        // before the "%", unlike Chase's adjacent "19.49%" format.
        let text = "Apple Card Customer\nJane Doe Statement\n\
                     Total Balance $10.00 \nas of Apr 30, 2026\n\
                     Interest Charges                        Interest Charge Calculation\n\
                     2026 Total Year-to-Date:                Annual Percentage Rate (APR) 14.49 % (variable)\n";

        let result = AppleCardStatementParser
            .parse(text, &expected(None))
            .unwrap();

        assert_eq!(result.apr, Some(14.49));
    }

    #[test]
    fn missing_apr_does_not_block_a_valid_balance() {
        let text = "Apple Card Customer\nJane Doe Statement\nTotal Balance $10.00\n";

        let result = AppleCardStatementParser
            .parse(text, &expected(None))
            .unwrap();

        assert_eq!(result.apr, None);
        assert_eq!(result.balance, 10.00);
    }
}
