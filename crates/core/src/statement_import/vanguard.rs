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
//!
//! **Holdings extraction (spec D31)**, verified against a real
//! Cash Plus/Brokerage statement (field labels/section headers only,
//! never a real balance/account number/name): two tables, both
//! optional (an empty result if neither is found isn't an error —
//! 529/Savings statements have neither).
//! - **`"Sweep program"`** — the account's cash position, read off the
//!   `"Total Sweep Balance $X $Y"` line rather than the single fund row
//!   above it (equivalent for the single-sweep-fund case this was
//!   verified against, and more robust to that row's own squished
//!   text-extraction layout).
//! - **`"Mutual funds"`** — one row per fund: `SYMBOL <qty> <price>
//!   <balance-on-date-1> <balance-on-date-2>NAME...` — note no
//!   separator between the last dollar amount and the fund's name; this
//!   is a `pdf-extract` column-ordering artifact, not a formatting
//!   choice, matching the same "numbers extracted before the label
//!   they belong to" pattern already seen on Chase's credit-card
//!   layout.
//!
//! Both tables show *two* dollar amounts per row/line (two different
//! `"Balance on"` dates). The **last** one is taken as current — this
//! statement consistently orders earlier-date-first, current-date-last
//! everywhere else it shows a two-point comparison (e.g. `"Value on
//! March ... / Value on April ..."` in the activity-summary section),
//! so the same convention is assumed to hold for the holdings tables.

use crate::statement_import::parser::{ExpectedAccount, ParseError, ParsedStatement, StatementParser};
use crate::{Category, Holding};

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
                    holdings: extract_holdings(text),
                    apr: None,
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

/// Combines the Sweep program's cash position (if present) with the
/// Mutual funds table's individual fund rows (if present) — spec D31.
/// Neither being found isn't an error; a 529/Savings-style statement
/// has neither table at all, and this parser is also used for those.
fn extract_holdings(text: &str) -> Vec<Holding> {
    let mut holdings: Vec<Holding> = extract_sweep_holding(text).into_iter().collect();
    holdings.extend(extract_mutual_fund_holdings(text));
    holdings
}

/// The `"Sweep program"` section's cash position, read off the
/// `"Total Sweep Balance $X $Y"` summary line — equivalent to summing
/// the section's own fund row(s) for the single-sweep-fund case this
/// was verified against, and more robust to that row's own squished
/// text-extraction layout (see module doc comment). Bounded to the same
/// line as the marker, then takes the *last* dollar amount on it — the
/// current-period one, per the module doc comment's chronological-
/// ordering convention.
fn extract_sweep_holding(text: &str) -> Option<Holding> {
    const MARKER: &str = "Total Sweep Balance ";
    let start = text.find(MARKER)? + MARKER.len();
    let line_end = text[start..]
        .find('\n')
        .map(|rel| start + rel)
        .unwrap_or(text.len());
    let value = last_dollar_amount_in(&text[start..line_end])?;

    Some(Holding {
        symbol: "SWEEP".into(),
        description: "Vanguard Federal Money Market Fund (Sweep)".into(),
        value,
    })
}

/// Parses the `"Mutual funds"` table, bounded so it never scans past
/// the next major section (`"Account activity"`/`"Completed
/// transactions"`) for symbol-shaped tokens that aren't actually fund
/// rows.
fn extract_mutual_fund_holdings(text: &str) -> Vec<Holding> {
    const SECTION_MARKER: &str = "Mutual funds";
    const SECTION_END_MARKERS: &[&str] = &["Account activity", "Completed transactions"];

    let Some(rel_start) = text.find(SECTION_MARKER) else {
        return Vec::new();
    };
    let section_start = rel_start + SECTION_MARKER.len();

    let section_end = SECTION_END_MARKERS
        .iter()
        .filter_map(|marker| text[section_start..].find(marker))
        .min()
        .map(|rel| section_start + rel)
        .unwrap_or(text.len());
    let section = &text[section_start..section_end];

    let symbol_positions = find_fund_symbol_positions(section);
    let mut holdings = Vec::new();

    for (sym_start, symbol) in &symbol_positions {
        // Bounded to *this row's own* nearest following blank line, not
        // "up to the next symbol or the section end" — the last fund in
        // the table has no next symbol to stop at, and would otherwise
        // run straight into the table's own totals line (a real bug
        // caught while writing this: `rfind('$')` would then grab the
        // totals line's dollar amount instead of this fund's own).
        let remaining = &section[*sym_start..];
        let row_end = remaining.find("\n\n").unwrap_or(remaining.len());
        let row_text = &remaining[..row_end];

        if let Some((value, name)) = extract_fund_value_and_name(row_text, symbol) {
            holdings.push(Holding {
                symbol: symbol.clone(),
                description: format!("{symbol} {name}"),
                value,
            });
        }
    }

    holdings
}

/// Finds every line starting with a 2-5 letter all-uppercase token
/// followed by a space then a digit — real Vanguard mutual fund tickers
/// (`VBIAX`, `VBTLX`, `VFIAX`, `VTIAX`, all 5 letters) match this;
/// requiring both the length bound and the trailing digit keeps this
/// from matching arbitrary capitalized words elsewhere in the section
/// (e.g. the table's own `"Symbol Name Quantity..."` header, which
/// isn't all-uppercase and so doesn't match at all).
fn find_fund_symbol_positions(section: &str) -> Vec<(usize, String)> {
    let mut positions = Vec::new();
    let mut offset = 0;

    for line in section.split_inclusive('\n') {
        let trimmed = line.trim_start();
        let leading_ws = line.len() - trimmed.len();
        let symbol: String = trimmed
            .chars()
            .take_while(|c| c.is_ascii_uppercase())
            .collect();

        if (2..=5).contains(&symbol.len()) {
            let after_symbol = &trimmed[symbol.len()..];
            let looks_like_a_row =
                after_symbol.starts_with(' ') && after_symbol.trim_start().starts_with(|c: char| c.is_ascii_digit());
            if looks_like_a_row {
                positions.push((offset + leading_ws, symbol));
            }
        }

        offset += line.len();
    }

    positions
}

/// Within one fund's row text (from its symbol up to the next symbol,
/// or the section end), finds the *last* dollar amount — the
/// current-period balance, per the module doc comment's chronological-
/// ordering convention (a fund row shows three amounts — price,
/// balance-on-date-1, balance-on-date-2 — so "last", not "second", is
/// the correct rule here) — and the fund's name, which the extracted
/// text runs directly against that amount with no separator (a
/// column-ordering artifact, not a real gap).
fn extract_fund_value_and_name(row_text: &str, symbol: &str) -> Option<(f64, String)> {
    let after_symbol = row_text.strip_prefix(symbol)?;
    let (value, amount_end) = last_dollar_amount_with_end_in(after_symbol)?;

    let name_region = &after_symbol[amount_end..];
    let name_end = name_region.find("\n\n").unwrap_or(name_region.len());
    let name: String = name_region[..name_end].split_whitespace().collect::<Vec<_>>().join(" ");

    (!name.is_empty()).then_some((value, name))
}

/// Finds the last `"$X"` in `text` and parses `X`, stripping
/// thousands-separator commas.
fn last_dollar_amount_in(text: &str) -> Option<f64> {
    last_dollar_amount_with_end_in(text).map(|(value, _)| value)
}

/// Same as [`last_dollar_amount_in`], but also returns the byte offset
/// immediately after the parsed amount's digits — callers that need to
/// keep reading the text past the amount (e.g. to find a fund's name
/// right after it) use this instead.
fn last_dollar_amount_with_end_in(text: &str) -> Option<(f64, usize)> {
    let last_dollar = text.rfind('$')?;
    let amount_start = last_dollar + 1;
    let amount: String = text[amount_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    if amount.is_empty() {
        return None;
    }
    let value = amount.replace(',', "").parse().ok()?;
    Some((value, amount_start + amount.len()))
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

    // --- Holdings extraction (spec D31), modeled on a real Cash
    // Plus/Brokerage statement's structure. Fund names/tickers below are
    // Vanguard's own public product names (same status as "Chase
    // Sapphire Reserve" elsewhere in this codebase) — only dollar
    // amounts and account/personal identifiers are ever synthetic.

    fn brokerage_statement_with_holdings() -> String {
        "Individual brokerage account\u{2014}XXXX5678\n\
         Account overview\n\
         $150,000.00\n\
         Total account value as of April 30, 2026\n\
         \n\
         Sweep program\n\
         \n\
         Name Quantity Price on\n01/01/2026 Balance on\n01/31/2026 Balance on\n04/30/2026\n\
         \n\
         1.0000 $1.00 $12,000.00 $2,000.00VANGUARD FEDERAL MONEY\n\
         MARKET FUND\n\
         1-day SEC Yield: 4.50%\n\
         \n\
         Total Sweep Balance $12,000.00 $2,000.00\n\
         \n\
         Mutual funds\n\
         \n\
         Symbol Name Quantity Price on\n01/01/2026 Balance on\n01/31/2026 Balance on\n04/30/2026\n\
         \n\
         VBIAX 100.0000 $40.00 $3,800.00 $4,000.00VANGUARD\n\
         BALANCED INDEX ADMIRAL CL\n\
         \n\
         VFIAX 50.0000 $500.00 $24,000.00 $25,000.00VANGUARD\n\
         500 INDEX ADMIRAL CL\n\
         \n\
         $27,800.00 $29,000.00\n\
         \n\
         Account activity for Vanguard Brokerage Account\u{2014}XXXX5678\n\
         \n\
         This section shows transactions that have settled by April 30, 2026.\n"
            .to_string()
    }

    #[test]
    fn extracts_the_sweep_program_cash_position_as_the_current_period_amount() {
        let text = brokerage_statement_with_holdings();
        let holdings = extract_holdings(&text);

        let sweep = holdings.iter().find(|h| h.symbol == "SWEEP").unwrap();
        assert_eq!(sweep.value, 2000.00);
        assert!(sweep.description.to_lowercase().contains("money market"));
    }

    #[test]
    fn extracts_each_mutual_fund_row_as_the_current_period_amount() {
        let text = brokerage_statement_with_holdings();
        let holdings = extract_holdings(&text);

        let vbiax = holdings.iter().find(|h| h.symbol == "VBIAX").unwrap();
        assert_eq!(vbiax.value, 4000.00);
        assert_eq!(vbiax.description, "VBIAX VANGUARD BALANCED INDEX ADMIRAL CL");

        let vfiax = holdings.iter().find(|h| h.symbol == "VFIAX").unwrap();
        assert_eq!(vfiax.value, 25000.00);
        assert_eq!(vfiax.description, "VFIAX VANGUARD 500 INDEX ADMIRAL CL");
    }

    #[test]
    fn mutual_fund_extraction_never_reads_past_the_account_activity_section() {
        let text = brokerage_statement_with_holdings();
        let holdings = extract_mutual_fund_holdings(&text);

        // Exactly the 2 real fund rows — nothing spuriously picked up
        // from the "Account activity" section or beyond.
        assert_eq!(holdings.len(), 2);
    }

    #[test]
    fn a_statement_with_no_holdings_tables_produces_no_holdings() {
        // The 529/Savings layout — no "Sweep program" or "Mutual funds"
        // section at all. Not an error; this parser handles both
        // layouts, and only one of them ever has holdings.
        let text = "Account number: XXXXX1234-01\n\
                     Account overview\n\
                     $50,000.00\n\
                     Total value of all accounts as of March 31, 2026\n";

        assert!(extract_holdings(text).is_empty());
    }

    #[test]
    fn parse_threads_holdings_through_into_the_parsed_statement() {
        let text = brokerage_statement_with_holdings();

        let result = VanguardStatementParser
            .parse(&text, &expected(None))
            .unwrap();

        assert_eq!(result.balance, 150000.00);
        assert_eq!(result.holdings.len(), 3); // sweep + 2 mutual funds
    }
}
