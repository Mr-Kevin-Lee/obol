//! Morgan Stanley / E*TRADE statement text parsing (spec §6.3, D32) —
//! verified against a real "Client Statement" (field labels/section
//! headers only, never a real balance/account number/name). Morgan
//! Stanley acquired E*TRADE in 2020; statements say "Morgan Stanley at
//! Work Self-Directed Account" and are accessed via etrade.com, so both
//! names dispatch to this parser (see `parser_for`).
//!
//! Structurally different from Vanguard/Chase in one important way:
//! **not every dollar amount on a row is the balance** — the `BALANCE
//! SHEET`'s `"TOTAL VALUE $X $Y"` line still follows the familiar
//! "earlier period, current period — take the last one" convention, but
//! an individual stock holding's row has *five* dollar amounts (Share
//! Price, Total Cost, Market Value, Unrealized Gain/Loss, Est Ann
//! Income, in that column order) and the one that matters — Market
//! Value — is the **third**, not the last. Verified against exactly one
//! real holding (a single stock, all five columns populated); a
//! statement with missing cost-basis data for some position (shown as
//! `"—"` per this statement's own disclosures) could shift this
//! counting — a known, stated limitation, not something to guess past.
//!
//! Deliberately excludes the `"STOCK PLAN DETAILS"` section (unvested/
//! potential RSU shares) from holdings — this statement's own
//! disclosure says plainly: *"The values for Stock Plan assets
//! displayed here do not represent assets held in your account and are
//! not protected by [SIPC]."* Counting them would overstate actual
//! current account value.

use crate::statement_import::parser::{ExpectedAccount, ParseError, ParsedStatement, StatementParser};
use crate::{Category, Holding};

pub struct MorganStanleyStatementParser;

impl StatementParser for MorganStanleyStatementParser {
    fn institution(&self) -> &'static str {
        "morganstanley"
    }

    fn parse(
        &self,
        text: &str,
        expected: &ExpectedAccount,
    ) -> Result<ParsedStatement, ParseError> {
        let Some(balance_section) = extract_balance_sheet_section(text) else {
            return Err(ParseError::UnrecognizedLayout(
                "no 'BALANCE SHEET' ... 'TOTAL VALUE $X $Y' section found".into(),
            ));
        };
        const TOTAL_VALUE_MARKER: &str = "TOTAL VALUE ";
        let Some(total_value_pos) = balance_section.find(TOTAL_VALUE_MARKER) else {
            return Err(ParseError::UnrecognizedLayout(
                "'BALANCE SHEET' section found but no 'TOTAL VALUE $X $Y' line in it".into(),
            ));
        };
        let after_marker = total_value_pos + TOTAL_VALUE_MARKER.len();
        let line_end = balance_section[after_marker..]
            .find('\n')
            .map(|rel| after_marker + rel)
            .unwrap_or(balance_section.len());
        let Some(balance) = last_dollar_amount_in(&balance_section[after_marker..line_end]) else {
            return Err(ParseError::UnrecognizedLayout(
                "'TOTAL VALUE' found but no dollar amount followed it".into(),
            ));
        };

        let Some(account_identifier) = extract_account_identifier(text) else {
            return Err(ParseError::UnrecognizedLayout(
                "no account-number line found after the account-type heading".into(),
            ));
        };

        if let Some(hint) = &expected.account_hint {
            if account_identifier != *hint {
                return Err(ParseError::NoMatchingAccount);
            }
        }

        Ok(ParsedStatement {
            balance,
            as_of_date: extract_as_of_date(balance_section).unwrap_or_default(),
            account_identifier,
            // Brokerage account — always an asset in this app's scope.
            // A margin debit balance could in principle make an
            // account net-negative, but this isn't handled (matches
            // Vanguard's same no-liability-products simplification);
            // the one real statement this was verified against shows
            // "Total Liabilities (outstanding balance) —" (zero).
            category: Category::Asset,
            holdings: extract_holdings(text),
            apr: None,
        })
    }
}

/// Bounds the `"BALANCE SHEET"` section (up to the next major section,
/// `"CASH FLOW"`) so the later `"TOTAL VALUE"` search can't accidentally
/// match the *different* `"TOTAL VALUE"` line in the earlier `"ASSET
/// ALLOCATION"` section (which has a different shape — one dollar
/// amount plus a percentage, not two dollar amounts).
fn extract_balance_sheet_section(text: &str) -> Option<&str> {
    const SECTION_MARKER: &str = "BALANCE SHEET";
    const SECTION_END_MARKER: &str = "CASH FLOW";

    let start = text.find(SECTION_MARKER)? + SECTION_MARKER.len();
    let end = text[start..]
        .find(SECTION_END_MARKER)
        .map(|rel| start + rel)
        .unwrap_or(text.len());
    Some(&text[start..end])
}

/// Finds the last `"$X"` in `text` and parses `X`, stripping
/// thousands-separator commas.
fn last_dollar_amount_in(text: &str) -> Option<f64> {
    let last_dollar = text.rfind('$')?;
    let amount_start = last_dollar + 1;
    let amount: String = text[amount_start..]
        .chars()
        .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
        .collect();
    if amount.is_empty() {
        return None;
    }
    amount.replace(',', "").parse().ok()
}

/// Finds the `n`th (1-indexed) `"$X"` in `text` and parses `X`.
fn nth_dollar_amount_in(text: &str, n: usize) -> Option<f64> {
    let mut search_from = 0;
    for i in 1..=n {
        let rel = text[search_from..].find('$')?;
        let dollar_pos = search_from + rel;
        if i == n {
            let amount_start = dollar_pos + 1;
            let amount: String = text[amount_start..]
                .chars()
                .take_while(|c| c.is_ascii_digit() || *c == ',' || *c == '.')
                .collect();
            return (!amount.is_empty())
                .then(|| amount.replace(',', ""))
                .and_then(|a| a.parse().ok());
        }
        search_from = dollar_pos + 1;
    }
    None
}

/// Best-effort as-of date, matching the last `"as of MM/DD/YY"` in the
/// given text — `"This Period"`'s date, not `"Last Period"`'s, per the
/// same last-occurrence-is-current convention used for the balance
/// itself. Returns `None` (not an error) if absent.
fn extract_as_of_date(text: &str) -> Option<String> {
    const MARKER: &str = "as of ";
    let mut last: Option<String> = None;
    let mut search_from = 0;

    while let Some(rel) = text[search_from..].find(MARKER) {
        let start = search_from + rel + MARKER.len();
        let raw: String = text[start..]
            .chars()
            .take_while(|c| c.is_ascii_digit() || *c == '/')
            .collect();
        if !raw.is_empty() {
            last = Some(raw);
        }
        search_from = start;
        if search_from >= text.len() {
            break;
        }
    }

    last
}

/// The account-number line (shaped like `"###-######-###"`) following
/// the account-type heading — verified against exactly one real
/// account-type label (`"Morgan Stanley at Work Self-Directed
/// Account"`); other Morgan Stanley/E*TRADE account types may use a
/// different label, a known, stated limitation rather than something
/// guessed past.
fn extract_account_identifier(text: &str) -> Option<String> {
    const MARKER: &str = "Morgan Stanley at Work Self-Directed Account";
    let start = text.find(MARKER)? + MARKER.len();
    let after = text[start..].trim_start_matches(['\n', '\r', ' ']);
    let line_end = after.find('\n').unwrap_or(after.len());
    let line = after[..line_end].trim();
    (!line.is_empty()).then(|| line.to_string())
}

/// Combines the cash/BDP/MMF summary with individual stock holdings —
/// spec D31/D32. Deliberately excludes `"STOCK PLAN DETAILS"` (see
/// module doc comment).
fn extract_holdings(text: &str) -> Vec<Holding> {
    let mut holdings: Vec<Holding> = extract_cash_holding(text).into_iter().collect();
    holdings.extend(extract_stock_holdings(text));
    holdings
}

/// The `"CASH, BDP, AND MMFs"` summary line — a single dollar amount,
/// unlike the stock rows below.
fn extract_cash_holding(text: &str) -> Option<Holding> {
    const MARKER: &str = "CASH, BDP, AND MMFs";
    let start = text.find(MARKER)? + MARKER.len();
    let line_end = text[start..].find('\n').map(|rel| start + rel).unwrap_or(text.len());
    let value = nth_dollar_amount_in(&text[start..line_end], 1)?;

    Some(Holding {
        symbol: "CASH".into(),
        description: "Cash, Bank Deposit Program, and Money Market Funds".into(),
        value,
    })
}

/// Parses the `"STOCKS"` / `"COMMON STOCKS"` section's individual
/// holding rows, bounded so it never reads into `"ALLOCATION OF
/// ASSETS"` or `"STOCK PLAN SUMMARY"`. Each row is `"NAME (TICKER)
/// <quantity> $<share price> $<total cost> $<market value>
/// $<unrealized gain/loss> $<est ann income> <yield>%"` — Market Value
/// is the third dollar amount, not the last (see module doc comment).
fn extract_stock_holdings(text: &str) -> Vec<Holding> {
    const SECTION_MARKER: &str = "COMMON STOCKS";
    const SECTION_END_MARKERS: &[&str] = &["ALLOCATION OF ASSETS", "STOCK PLAN SUMMARY"];

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

    let mut holdings = Vec::new();
    for line in section.lines() {
        let Some(paren_start) = line.find(" (") else {
            continue;
        };
        let after_paren_open = &line[paren_start + 2..];
        let Some(paren_end) = after_paren_open.find(')') else {
            continue;
        };
        let ticker = &after_paren_open[..paren_end];
        let is_ticker = (1..=5).contains(&ticker.len()) && ticker.chars().all(|c| c.is_ascii_uppercase());
        if !is_ticker {
            continue;
        }

        let name = line[..paren_start].trim();
        if name.is_empty() {
            continue;
        }
        let rest = &after_paren_open[paren_end + 1..];
        let Some(value) = nth_dollar_amount_in(rest, 3) else {
            continue;
        };

        holdings.push(Holding {
            symbol: ticker.to_string(),
            description: name.to_string(),
            value,
        });
    }

    holdings
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

    fn statement_with_holdings() -> String {
        "Morgan Stanley at Work Self-Directed Account\n\
         123-456789-012\n\
         \n\
         BALANCE SHEET (^ includes accrued interest) Last Period\n\
         (as of 05/31/26) This Period\n\
         (as of 06/30/26)\n\
         Cash, BDP, MMFs $1.00 $1.00\n\
         \n\
         Stocks 190,000.00 200,000.00\n\
         Total Assets $190,001.00 $200,001.00\n\
         \n\
         Total Liabilities  (outstanding balance) \u{2014} \u{2014}\n\
         \n\
         TOTAL VALUE $190,001.00 $200,001.00\n\
         CASH FLOW This Period\n\
         (6/1/26-6/30/26) This Year\n\
         (1/1/26-6/30/26)\n\
         \n\
         COMMON STOCKS\n\
         \n\
         Security Description Quantity Share Price Total Cost Market Value Unrealized\n\
         Gain/(Loss) Est Ann Income Current\n\
         Yield %\n\
         APPLE INC (AAPL)  1,000.000 $200.000 $150,000.00 $200,000.00 $50,000.00 $500.00 0.25\n\
         Rating: Morgan Stanley: 1, Morningstar: 4\n\
         \n\
         Percentage\n\
         of Holdings Total Cost Market Value Unrealized\n\
         Gain/(Loss) Est Ann Income Current\n\
         Yield %\n\
         STOCKS 100.00% $150,000.00 $200,000.00 $50,000.00 $500.00 0.25%\n\
         \n\
         Percentage\n\
         of Holdings Market Value Est Ann Income\n\
         CASH, BDP, AND MMFs 0.50% $1.00 \u{2014}\n\
         \n\
         ALLOCATION OF ASSETS\n\
         Cash Equities\n\
         TOTAL ALLOCATION OF ASSETS $1.00 $200,000.00\n\
         \n\
         STOCK PLAN SUMMARY As of 6/30/2026\n\
         \n\
         STOCK PLAN DETAILS\n\
         \n\
         Grant Date Number Type Symbol/CUSIP Potential Quantity Grant Price Market Price Total Est Mkt Value\n\
         01/01/24 999999 RSU AAPL 50.000 $0.00 $200.00 $10,000.00\n"
            .to_string()
    }

    #[test]
    fn parses_the_current_period_total_value() {
        let text = statement_with_holdings();

        let result = MorganStanleyStatementParser
            .parse(&text, &expected(None))
            .unwrap();

        assert_eq!(result.balance, 200001.00);
        assert_eq!(result.as_of_date, "06/30/26");
        assert_eq!(result.account_identifier, "123-456789-012");
        assert_eq!(result.category, Category::Asset);
    }

    #[test]
    fn extracts_the_cash_position() {
        let text = statement_with_holdings();
        let holdings = extract_holdings(&text);

        let cash = holdings.iter().find(|h| h.symbol == "CASH").unwrap();
        assert_eq!(cash.value, 1.00);
    }

    #[test]
    fn extracts_a_stock_holdings_market_value_not_share_price_or_total_cost() {
        let text = statement_with_holdings();
        let holdings = extract_holdings(&text);

        let aapl = holdings.iter().find(|h| h.symbol == "AAPL").unwrap();
        assert_eq!(aapl.description, "APPLE INC");
        // Market Value ($200,000.00), not Share Price ($200.000) or
        // Total Cost ($150,000.00) — the whole point of this test.
        assert_eq!(aapl.value, 200000.00);
    }

    #[test]
    fn stock_extraction_never_reads_past_allocation_of_assets() {
        let text = statement_with_holdings();
        let holdings = extract_stock_holdings(&text);
        assert_eq!(holdings.len(), 1);
    }

    #[test]
    fn stock_plan_details_rsu_rows_are_never_counted_as_holdings() {
        let text = statement_with_holdings();
        let holdings = extract_holdings(&text);

        // Only CASH + AAPL (from the actual STOCKS section) — the RSU
        // row in STOCK PLAN DETAILS is unvested/potential value, not
        // actual account value, per this statement's own disclosure.
        assert_eq!(holdings.len(), 2);
    }

    #[test]
    fn account_hint_matches_the_full_account_number() {
        let text = statement_with_holdings();

        let result = MorganStanleyStatementParser
            .parse(&text, &expected(Some("123-456789-012")))
            .unwrap();

        assert_eq!(result.balance, 200001.00);
    }

    #[test]
    fn errors_when_the_account_hint_does_not_match() {
        let text = statement_with_holdings();

        let err = MorganStanleyStatementParser
            .parse(&text, &expected(Some("999-999999-999")))
            .unwrap_err();

        assert_eq!(err, ParseError::NoMatchingAccount);
    }

    #[test]
    fn errors_on_text_with_no_recognizable_layout() {
        let text = "this text does not look like a Morgan Stanley statement at all";

        let err = MorganStanleyStatementParser
            .parse(text, &expected(None))
            .unwrap_err();

        assert!(matches!(err, ParseError::UnrecognizedLayout(_)));
    }
}
