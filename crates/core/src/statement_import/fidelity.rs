//! Fidelity NetBenefits ("Statement Details") statement text parsing
//! (spec §6.3, D28). Structurally simpler than Chase or Vanguard in one
//! respect and harder in another:
//!
//! - Simpler: a single, unambiguous balance marker — `"Ending Balance
//!   $X"` — distinct from the also-present `"Beginning Balance $X"` and
//!   `"Vested Balance $X"` lines, so no nearest-following heuristic is
//!   needed to pick the right dollar figure.
//! - Harder: there is no account number anywhere in the document. The
//!   only identifier is a plan heading shaped `"[Employer] 401(k)
//!   Plan"` near the top (e.g. an employer name immediately followed by
//!   `"401(k) Plan"`). `ExpectedAccount.account_hint` is therefore
//!   interpreted as a case-insensitive substring to match against that
//!   heading, not a last-4 digit string.
//!
//! Statement date comes from `"Statement Period: [start] to [end]"`
//! (`MM/DD/YYYY`); the *end* of the period is used as the as-of date,
//! since that's the date the ending balance is as-of.

use crate::statement_import::parser::{ExpectedAccount, ParseError, ParsedStatement, StatementParser};

pub struct FidelityStatementParser;

impl StatementParser for FidelityStatementParser {
    fn institution(&self) -> &'static str {
        "fidelity"
    }

    fn parse(
        &self,
        text: &str,
        expected: &ExpectedAccount,
    ) -> Result<ParsedStatement, ParseError> {
        let plans = extract_plan_sections(text);
        if plans.is_empty() {
            return Err(ParseError::UnrecognizedLayout(
                "no '[Employer] 401(k) Plan' heading found".into(),
            ));
        }

        let matches: Vec<&PlanSection> = plans
            .iter()
            .filter(|p| match &expected.account_hint {
                Some(hint) => p.heading.to_lowercase().contains(&hint.to_lowercase()),
                None => true,
            })
            .collect();

        match matches.len() {
            0 => Err(ParseError::NoMatchingAccount),
            1 => {
                let plan = matches[0];
                Ok(ParsedStatement {
                    balance: plan.ending_balance,
                    as_of_date: extract_statement_period_end(text).unwrap_or_default(),
                    account_identifier: plan.heading.clone(),
                })
            }
            _ => Err(ParseError::AmbiguousMatch),
        }
    }
}

struct PlanSection {
    heading: String,
    ending_balance: f64,
}

/// Finds every `"... 401(k) Plan"` heading and the `"Ending Balance $X"`
/// that follows it, same nearest-following pairing convention as
/// `chase.rs`/`vanguard.rs`.
fn extract_plan_sections(text: &str) -> Vec<PlanSection> {
    const MARKER: &str = "401(k) Plan";
    let mut sections = Vec::new();
    let mut search_from = 0;

    while let Some(rel_pos) = text[search_from..].find(MARKER) {
        let marker_start = search_from + rel_pos;
        let marker_end = marker_start + MARKER.len();
        let heading = extract_heading_line(&text[..marker_end]);

        if let Some(balance) = find_next_ending_balance(&text[marker_end..]) {
            sections.push(PlanSection { heading, ending_balance: balance });
        }

        search_from = marker_end;
        if search_from >= text.len() {
            break;
        }
    }

    sections
}

/// Walks backward from the end of `"... 401(k) Plan"` to the start of
/// that line, so `heading` is the full `"[Employer] 401(k) Plan"` text
/// rather than just the marker itself.
fn extract_heading_line(text_up_to_marker_end: &str) -> String {
    let line_start = text_up_to_marker_end
        .rfind('\n')
        .map(|pos| pos + 1)
        .unwrap_or(0);
    text_up_to_marker_end[line_start..].trim().to_string()
}

/// Finds the next `"Ending Balance $X"` after the given position and
/// parses `X` as a decimal amount, stripping thousands-separator
/// commas. Deliberately distinct from `"Beginning Balance"` and
/// `"Vested Balance"`, which also appear in these statements but aren't
/// this parser's target figure.
fn find_next_ending_balance(text: &str) -> Option<f64> {
    const MARKER: &str = "Ending Balance $";
    let amount_start = text.find(MARKER)? + MARKER.len();
    let amount: String = text[amount_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    amount.replace(',', "").parse().ok()
}

/// Best-effort as-of date: the *end* of `"Statement Period: [start] to
/// [end]"`. Returns `None` (not an error) if absent, same
/// missing-date-doesn't-block-a-balance rule as `chase.rs`.
fn extract_statement_period_end(text: &str) -> Option<String> {
    const MARKER: &str = "Statement Period: ";
    let start = text.find(MARKER)? + MARKER.len();
    const TO_MARKER: &str = " to ";
    let to_pos = text[start..].find(TO_MARKER)? + TO_MARKER.len();
    let end_start = start + to_pos;
    let raw: String = text[end_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == '/')
        .collect();
    (!raw.is_empty()).then_some(raw)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn expected(account_hint: Option<&str>) -> ExpectedAccount {
        ExpectedAccount {
            account_type: "401k".into(),
            account_hint: account_hint.map(String::from),
        }
    }

    #[test]
    fn parses_a_single_plan() {
        let text = "Statement Details\n\
                     Acme Corp 401(k) Plan\n\
                     Your Account Summary\n\
                     Statement Period: 01/01/2026 to 03/31/2026\n\
                     Beginning Balance $9,000.00\n\
                     Ending Balance $10,000.00\n\
                     Vested Balance $8,500.00\n";

        let result = FidelityStatementParser
            .parse(text, &expected(None))
            .unwrap();

        assert_eq!(result.balance, 10000.00);
        assert_eq!(result.as_of_date, "03/31/2026");
        assert_eq!(result.account_identifier, "Acme Corp 401(k) Plan");
    }

    #[test]
    fn disambiguates_multiple_plans_via_account_hint() {
        let text = "Acme Corp 401(k) Plan\n\
                     Ending Balance $10,000.00\n\
                     Widget Inc 401(k) Plan\n\
                     Ending Balance $20,000.00\n";

        let result = FidelityStatementParser
            .parse(text, &expected(Some("widget")))
            .unwrap();

        assert_eq!(result.balance, 20000.00);
        assert_eq!(result.account_identifier, "Widget Inc 401(k) Plan");
    }

    #[test]
    fn errors_when_no_plan_matches_the_given_hint() {
        let text = "Acme Corp 401(k) Plan\nEnding Balance $10,000.00\n";

        let err = FidelityStatementParser
            .parse(text, &expected(Some("nonexistent employer")))
            .unwrap_err();

        assert_eq!(err, ParseError::NoMatchingAccount);
    }

    #[test]
    fn errors_when_multiple_plans_match_and_no_hint_was_given() {
        let text = "Acme Corp 401(k) Plan\n\
                     Ending Balance $10,000.00\n\
                     Widget Inc 401(k) Plan\n\
                     Ending Balance $20,000.00\n";

        let err = FidelityStatementParser
            .parse(text, &expected(None))
            .unwrap_err();

        assert_eq!(err, ParseError::AmbiguousMatch);
    }

    #[test]
    fn errors_on_text_with_no_recognizable_fidelity_layout() {
        let text = "this text does not look like a Fidelity statement at all";

        let err = FidelityStatementParser
            .parse(text, &expected(None))
            .unwrap_err();

        assert!(matches!(err, ParseError::UnrecognizedLayout(_)));
    }

    #[test]
    fn missing_statement_period_does_not_block_a_valid_balance() {
        let text = "Acme Corp 401(k) Plan\nEnding Balance $10,000.00\n";

        let result = FidelityStatementParser
            .parse(text, &expected(None))
            .unwrap();

        assert_eq!(result.balance, 10000.00);
        assert_eq!(result.as_of_date, "");
    }
}
