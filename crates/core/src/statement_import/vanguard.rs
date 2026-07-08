//! Vanguard-specific statement text parsing (spec §6.3, D28). Vanguard
//! ships at least two distinct statement layouts depending on account
//! type, both handled by this one parser:
//!
//! - **529/Savings-style**: a single `"Account overview"` section, an
//!   account number shaped `"Account number: XXXXX####-##"` (five
//!   literal `X`s, then digits, then a `-NN` suffix), and either
//!   `"Total value of all accounts as of [Month DD, YYYY]"` right after
//!   the overview's dollar figure, or an `"Ending balance on [date]
//!   $[amount]"` line in an "Account activity" section.
//! - **Cash Plus/Brokerage-style**: TWO summary sections in the same
//!   document — a top-level, multi-account `"Statement overview"` (must
//!   be ignored; it's not this source's balance) and a per-account
//!   `"Account overview"` (the correct one, same heading text as the
//!   529 layout above). Account id is shaped
//!   `"[Account Type] account\u{2014}XXXX####"` (em dash, four literal
//!   `X`s, then digits, no suffix). Also carries a `"Total account value
//!   as of [date]"` variant of the total-value line, and an "Activity
//!   summary for statement period" block with `"Value on [date]
//!   $[amount]"` lines.
//!
//! Both layouts are handled by finding every `"Account overview"`
//! occurrence (never `"Statement overview"`) and reading the nearest
//! following dollar amount as that section's balance — the same
//! nearest-following heuristic `chase.rs` uses, which holds here too
//! since Vanguard always pairs a section heading with its own total
//! immediately after it.

use crate::statement_import::parser::{ExpectedAccount, ParseError, ParsedStatement, StatementParser};
use crate::Category;

pub struct VanguardStatementParser;

impl StatementParser for VanguardStatementParser {
    fn institution(&self) -> &'static str {
        "vanguard"
    }

    fn parse(
        &self,
        text: &str,
        expected: &ExpectedAccount,
    ) -> Result<ParsedStatement, ParseError> {
        let sections = extract_account_overview_sections(text);
        if sections.is_empty() {
            return Err(ParseError::UnrecognizedLayout(
                "no 'Account overview' section found".into(),
            ));
        }

        let matches: Vec<&AccountSection> = sections
            .iter()
            .filter(|s| match &expected.account_hint {
                Some(hint) => s.account_id.as_deref() == Some(hint.as_str()),
                None => true,
            })
            .collect();

        match matches.len() {
            0 => Err(ParseError::NoMatchingAccount),
            1 => {
                let section = matches[0];
                Ok(ParsedStatement {
                    balance: section.balance,
                    as_of_date: extract_as_of_date(text).unwrap_or_default(),
                    account_identifier: section
                        .account_id
                        .clone()
                        .unwrap_or_else(|| section.balance.to_string()),
                    // Vanguard has no liability products in this app's
                    // scope (spec FR1: brokerage/529/money-market are
                    // always assets) — no statement inspection needed.
                    category: Category::Asset,
                })
            }
            _ => Err(ParseError::AmbiguousMatch),
        }
    }
}

struct AccountSection {
    account_id: Option<String>,
    balance: f64,
}

/// Finds every `"Account overview"` heading (explicitly never
/// `"Statement overview"`, which is the multi-account top-level total
/// on the Cash Plus/Brokerage layout) and reads the nearest following
/// dollar amount as that section's balance, and the nearest *preceding*
/// account-number marker (either the 529-style `"Account number:
/// XXXXX####-##"` or the Cash Plus-style em-dash `"...account\u{2014}XXXX####"`)
/// as its identifier.
fn extract_account_overview_sections(text: &str) -> Vec<AccountSection> {
    const MARKER: &str = "Account overview";
    let mut sections = Vec::new();
    let mut search_from = 0;

    while let Some(rel_pos) = text[search_from..].find(MARKER) {
        let heading_pos = search_from + rel_pos;
        let after_heading = heading_pos + MARKER.len();

        if let Some(balance) = find_next_dollar_amount(&text[after_heading..]) {
            let account_id = find_nearest_account_id(&text[..heading_pos]);
            sections.push(AccountSection { account_id, balance });
        }

        search_from = after_heading;
        if search_from >= text.len() {
            break;
        }
    }

    sections
}

/// Finds the next `"$X"` after the given position and parses `X` as a
/// decimal amount, stripping thousands-separator commas.
fn find_next_dollar_amount(text: &str) -> Option<f64> {
    let dollar_pos = text.find('$')?;
    let amount_start = dollar_pos + 1;
    let amount: String = text[amount_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    if amount.is_empty() {
        return None;
    }
    amount.replace(',', "").parse().ok()
}

/// Looks backward from a section heading for the closest account
/// identifier marker, trying both known Vanguard layouts. Backward
/// search (rather than forward from the top of the document) so that
/// on a multi-account statement, each `"Account overview"` picks up
/// *its own* account number rather than always the first one found.
fn find_nearest_account_id(preceding_text: &str) -> Option<String> {
    let five_x = find_last_account_number_field(preceding_text);
    let em_dash = find_last_em_dash_account(preceding_text);

    match (five_x, em_dash) {
        (Some((pos_a, id_a)), Some((pos_b, id_b))) => {
            if pos_a >= pos_b {
                Some(id_a)
            } else {
                Some(id_b)
            }
        }
        (Some((_, id)), None) => Some(id),
        (None, Some((_, id))) => Some(id),
        (None, None) => None,
    }
}

/// 529/Savings-style: `"Account number: XXXXX####-##"`.
fn find_last_account_number_field(text: &str) -> Option<(usize, String)> {
    const MARKER: &str = "Account number: ";
    let mut last: Option<(usize, String)> = None;
    let mut search_from = 0;

    while let Some(rel_pos) = text[search_from..].find(MARKER) {
        let id_start = search_from + rel_pos + MARKER.len();
        let id: String = text[id_start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric() || *c == '-')
            .collect();
        if !id.is_empty() {
            last = Some((id_start, id));
        }
        search_from = id_start;
        if search_from >= text.len() {
            break;
        }
    }

    last
}

/// Cash Plus/Brokerage-style: `"...account\u{2014}XXXX####"` (em dash,
/// four literal `X`s, then digits).
fn find_last_em_dash_account(text: &str) -> Option<(usize, String)> {
    const MARKER: &str = "account\u{2014}";
    let mut last: Option<(usize, String)> = None;
    let mut search_from = 0;

    while let Some(rel_pos) = text[search_from..].find(MARKER) {
        let id_start = search_from + rel_pos + MARKER.len();
        let id: String = text[id_start..]
            .chars()
            .take_while(|c| c.is_ascii_alphanumeric())
            .collect();
        if !id.is_empty() {
            last = Some((id_start, id));
        }
        search_from = id_start;
        if search_from >= text.len() {
            break;
        }
    }

    last
}

/// Best-effort as-of date, matching either `"Total value of all
/// accounts as of [Month DD, YYYY]"` or `"Total account value as of
/// [date]"`. Returns `None` (not an error) if absent, same
/// missing-date-doesn't-block-a-balance rule as `chase.rs`.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn expected(account_hint: Option<&str>) -> ExpectedAccount {
        ExpectedAccount {
            account_type: "brokerage".into(),
            account_hint: account_hint.map(String::from),
        }
    }

    #[test]
    fn parses_529_style_account_overview() {
        let text = "Account summary\n\
                     Account number: XXXXX1234-01\n\
                     Account overview\n\
                     $50,000.00\n\
                     Total value of all accounts as of March 31, 2026\n";

        let result = VanguardStatementParser
            .parse(text, &expected(None))
            .unwrap();

        assert_eq!(result.balance, 50000.00);
        assert_eq!(result.as_of_date, "March 31, 2026");
        assert_eq!(result.account_identifier, "XXXXX1234-01");
        assert_eq!(result.category, Category::Asset);
    }

    #[test]
    fn parses_cash_plus_style_and_ignores_statement_overview() {
        let text = "Statement overview\n\
                     $99,999.99\n\
                     Total value of all accounts as of April 30, 2026\n\
                     Cash Plus account\u{2014}XXXX5678\n\
                     Account overview\n\
                     $12,345.67\n\
                     Total account value as of April 30, 2026\n";

        let result = VanguardStatementParser
            .parse(text, &expected(None))
            .unwrap();

        assert_eq!(result.balance, 12345.67);
        assert_eq!(result.account_identifier, "XXXX5678");
    }

    #[test]
    fn disambiguates_multiple_accounts_via_account_hint() {
        let text = "Account number: XXXXX1111-01\n\
                     Account overview\n\
                     $1,000.00\n\
                     Total value of all accounts as of March 31, 2026\n\
                     Account number: XXXXX2222-01\n\
                     Account overview\n\
                     $2,000.00\n\
                     Total value of all accounts as of March 31, 2026\n";

        let result = VanguardStatementParser
            .parse(text, &expected(Some("XXXXX2222-01")))
            .unwrap();

        assert_eq!(result.balance, 2000.00);
        assert_eq!(result.account_identifier, "XXXXX2222-01");
    }

    #[test]
    fn errors_when_no_account_matches_the_given_hint() {
        let text = "Account number: XXXXX1111-01\n\
                     Account overview\n\
                     $1,000.00\n";

        let err = VanguardStatementParser
            .parse(text, &expected(Some("XXXXX9999-01")))
            .unwrap_err();

        assert_eq!(err, ParseError::NoMatchingAccount);
    }

    #[test]
    fn errors_when_multiple_accounts_match_and_no_hint_was_given() {
        let text = "Account number: XXXXX1111-01\n\
                     Account overview\n\
                     $1,000.00\n\
                     Account number: XXXXX2222-01\n\
                     Account overview\n\
                     $2,000.00\n";

        let err = VanguardStatementParser
            .parse(text, &expected(None))
            .unwrap_err();

        assert_eq!(err, ParseError::AmbiguousMatch);
    }

    #[test]
    fn errors_on_text_with_no_recognizable_vanguard_layout() {
        let text = "this text does not look like a Vanguard statement at all";

        let err = VanguardStatementParser
            .parse(text, &expected(None))
            .unwrap_err();

        assert!(matches!(err, ParseError::UnrecognizedLayout(_)));
    }

    #[test]
    fn missing_as_of_date_does_not_block_a_valid_balance() {
        let text = "Account overview\n$1,000.00\n";

        let result = VanguardStatementParser
            .parse(text, &expected(None))
            .unwrap();

        assert_eq!(result.balance, 1000.00);
        assert_eq!(result.as_of_date, "");
    }
}
