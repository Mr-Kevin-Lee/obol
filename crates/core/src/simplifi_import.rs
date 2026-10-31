//! The one-time Simplifi CSV import (spec §11.4, D44): backfills history
//! from Simplifi's transactions export and seeds `merchant_rules.yaml` with
//! the category used most for each merchant. Local only; an export is never
//! committed and its payees never reach a log line or an error message.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::io;
use std::path::Path;

use thiserror::Error;
use time::{Date, Month};

use crate::categorize::{INCOME, TRANSFER, UNCATEGORIZED};
use crate::merchant::merchant_key;
use crate::merchant_rules_storage::{seed_merchant_rules, MerchantRulesError};
use crate::transaction::{format_iso_date, Transaction};
use crate::transaction_storage::{save_transactions, SaveSummary, TransactionStorageError};

#[derive(Debug, Error)]
pub enum SimplifiImportError {
    #[error("the export could not be read: {0}")]
    Io(#[from] io::Error),
    #[error("not a Simplifi transactions export: it has no {0:?} column")]
    MissingColumn(&'static str),
    #[error("the export ends inside a quoted field")]
    UnterminatedQuote,
    /// `row` counts data rows from 1, after the header.
    #[error("row {row}: {problem}")]
    Row { row: usize, problem: String },
    #[error(transparent)]
    Transactions(#[from] TransactionStorageError),
    #[error(transparent)]
    MerchantRules(#[from] MerchantRulesError),
}

/// A parsed export, not yet saved.
#[derive(Debug, Clone, PartialEq)]
pub struct SimplifiExport {
    pub transactions: Vec<Transaction>,
    /// Merchant key → the category Simplifi used most for it.
    pub rules: BTreeMap<String, String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SimplifiImportSummary {
    pub transactions: SaveSummary,
    pub rules_added: usize,
}

/// Simplifi's own names for a move between accounts; it also uses an
/// account's name as the category of a transfer into it.
const TRANSFER_CATEGORIES: &[&str] = &["Transfer", "Credit Card Payment", "Balance Adjustment"];

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

pub fn parse_simplifi_export(text: &str) -> Result<SimplifiExport, SimplifiImportError> {
    let mut records = records(text)?.into_iter();
    let header = records.next().unwrap_or_default();
    let column = |name: &'static str| {
        header
            .iter()
            .position(|field| field == name)
            .ok_or(SimplifiImportError::MissingColumn(name))
    };
    let (date, account, payee, category, exclusion, amount) = (
        column("Date")?,
        column("Account")?,
        column("Payee")?,
        column("Category")?,
        column("Exclusion")?,
        column("Amount")?,
    );
    let rows: Vec<Vec<String>> = records.collect();
    let accounts: BTreeSet<String> = rows
        .iter()
        .filter_map(|row| row.get(account))
        .map(|name| collapse_spaces(name))
        .collect();

    let mut occurrences: HashMap<(String, String), usize> = HashMap::new();
    let mut votes: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    let mut transactions = Vec::with_capacity(rows.len());
    for (index, row) in rows.iter().enumerate() {
        // Problems name the row, never its payee or amount.
        let bad = |problem: String| SimplifiImportError::Row {
            row: index + 1,
            problem,
        };
        if row.len() != header.len() {
            return Err(bad(format!(
                "{} fields, expected {}",
                row.len(),
                header.len()
            )));
        }
        let posted =
            parse_date(&row[date]).ok_or_else(|| bad(format!("invalid date {:?}", row[date])))?;
        let amount = parse_amount(&row[amount]).ok_or_else(|| bad("invalid amount".into()))?;
        let source_id = format!("simplifi:{}", collapse_spaces(&row[account]));
        let description = row[payee].clone();

        // Simplifi exports no ids. Identical rows are told apart by their
        // order, so an id stays the same across exports as long as no new
        // identical row appears before it.
        let base = format!("{} {amount:.2} {description}", format_iso_date(posted));
        let seen = occurrences
            .entry((source_id.clone(), base.clone()))
            .or_default();
        *seen += 1;

        let key = merchant_key(&description);
        if let Some(category) = obol_category(&row[category], row[exclusion] == "yes", &accounts) {
            if !key.is_empty() {
                *votes.entry(key).or_default().entry(category).or_default() += 1;
            }
        }
        transactions.push(Transaction {
            id: format!("{base} #{seen}"),
            source_id,
            posted,
            amount,
            description,
        });
    }

    let rules = votes
        .into_iter()
        .filter_map(|(key, counts)| {
            // Most votes wins; a tie goes to the alphabetically first.
            let (category, _) = counts
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))?;
            Some((key, category))
        })
        .collect();
    Ok(SimplifiExport {
        transactions,
        rules,
    })
}

/// Parses `export` and saves it. Additive: a re-import of the same or a
/// later export adds only rows not yet stored (an id is built from every
/// field, so a stored row never changes), and seeds only merchants with no
/// rule, so earlier seeds and edits in the app stay. A bad row fails the
/// whole import before anything is written.
pub fn import_simplifi_export(
    export: &Path,
    transactions_dir: &Path,
    merchant_rules: &Path,
) -> Result<SimplifiImportSummary, SimplifiImportError> {
    let parsed = parse_simplifi_export(&fs::read_to_string(export)?)?;
    Ok(SimplifiImportSummary {
        transactions: save_transactions(transactions_dir, &parsed.transactions)?,
        rules_added: seed_merchant_rules(merchant_rules, &parsed.rules)?,
    })
}

/// RFC 4180 records: quoted fields may hold commas, newlines and doubled
/// quotes. Blank lines are skipped.
fn records(text: &str) -> Result<Vec<Vec<String>>, SimplifiImportError> {
    let text = text.strip_prefix('\u{feff}').unwrap_or(text);
    let (mut records, mut record, mut field) = (Vec::new(), Vec::new(), String::new());
    let mut quoted = false;
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        match (quoted, c) {
            (true, '"') if chars.peek() == Some(&'"') => {
                chars.next();
                field.push('"');
            }
            (true, '"') => quoted = false,
            (true, _) => field.push(c),
            (false, '"') => quoted = true,
            (false, ',') => record.push(std::mem::take(&mut field)),
            (false, '\r') if chars.peek() == Some(&'\n') => {}
            (false, '\n') => {
                record.push(std::mem::take(&mut field));
                records.push(std::mem::take(&mut record));
            }
            (false, _) => field.push(c),
        }
    }
    if quoted {
        return Err(SimplifiImportError::UnterminatedQuote);
    }
    if !field.is_empty() || !record.is_empty() {
        record.push(field);
        records.push(record);
    }
    records.retain(|record| record != &[""]);
    Ok(records)
}

/// Exactly `Mon D, YYYY`, as Simplifi writes it.
fn parse_date(text: &str) -> Option<Date> {
    let (month_day, year) = text.split_once(", ")?;
    let (month, day) = month_day.split_once(' ')?;
    let month = MONTHS.iter().position(|name| *name == month)? + 1;
    if !is_digits(day, 1..=2) || !is_digits(year, 4..=4) {
        return None;
    }
    let month = Month::try_from(u8::try_from(month).ok()?).ok()?;
    Date::from_calendar_date(year.parse().ok()?, month, day.parse().ok()?).ok()
}

fn is_digits(text: &str, len: std::ops::RangeInclusive<usize>) -> bool {
    len.contains(&text.len()) && text.bytes().all(|byte| byte.is_ascii_digit())
}

fn parse_amount(text: &str) -> Option<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|amount| amount.is_finite())
}

/// The obol category for a Simplifi one, or `None` for a row that shouldn't
/// vote on its merchant's rule.
fn obol_category(category: &str, excluded: bool, accounts: &BTreeSet<String>) -> Option<String> {
    let category = collapse_spaces(category);
    if excluded || TRANSFER_CATEGORIES.contains(&category.as_str()) || accounts.contains(&category)
    {
        return Some(TRANSFER.into());
    }
    let parent = category
        .split_once(':')
        .map_or(category.as_str(), |(parent, _)| parent);
    match parent {
        "" | UNCATEGORIZED => None,
        "Utilities" => Some("Bills & Utilities"),
        "Personal Income" => Some(INCOME),
        "Fees & Charges" => Some("Fees"),
        other => Some(other),
    }
    .map(String::from)
}

/// Simplifi writes some account names with non-breaking spaces.
fn collapse_spaces(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const HEADER: &str = "Date,Account,Reviewed,Payee,Category,Exclusion,Amount";

    fn export(rows: &[&str]) -> String {
        std::iter::once(HEADER)
            .chain(rows.iter().copied())
            .map(|line| format!("{line}\n"))
            .collect()
    }

    fn parsed(rows: &[&str]) -> SimplifiExport {
        parse_simplifi_export(&export(rows)).unwrap()
    }

    fn accounts(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn date(year: i32, month: Month, day: u8) -> Date {
        Date::from_calendar_date(year, month, day).unwrap()
    }

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "obol-simplifi-import-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    // --- CSV records (RFC 4180) ---

    #[test]
    fn should_split_a_line_into_fields() {
        assert_eq!(records("a,b,c\n").unwrap(), [["a", "b", "c"]]);
    }

    #[test]
    fn should_keep_a_comma_inside_quotes() {
        assert_eq!(
            records("\"Oct 27, 2026\",x\n").unwrap(),
            [["Oct 27, 2026", "x"]]
        );
    }

    #[test]
    fn should_unescape_doubled_quotes() {
        assert_eq!(records("\"say \"\"hi\"\"\"\n").unwrap(), [["say \"hi\""]]);
    }

    #[test]
    fn should_keep_a_newline_inside_quotes() {
        assert_eq!(records("\"a\nb\",c\n").unwrap(), [["a\nb", "c"]]);
    }

    #[test]
    fn should_accept_crlf_line_endings() {
        assert_eq!(records("a,b\r\nc,d\r\n").unwrap(), [["a", "b"], ["c", "d"]]);
    }

    #[test]
    fn should_read_a_last_line_without_a_newline() {
        assert_eq!(records("a,b\nc,d").unwrap(), [["a", "b"], ["c", "d"]]);
    }

    #[test]
    fn should_read_a_single_field_last_line_without_a_newline() {
        assert_eq!(records("a\nb").unwrap(), [["a"], ["b"]]);
    }

    #[test]
    fn should_keep_an_empty_last_field() {
        assert_eq!(records("a,\n").unwrap(), [["a", ""]]);
    }

    #[test]
    fn should_skip_blank_lines() {
        assert_eq!(records("a\n\nb\n").unwrap(), [["a"], ["b"]]);
    }

    #[test]
    fn should_strip_a_byte_order_mark() {
        assert_eq!(records("\u{feff}a\n").unwrap(), [["a"]]);
    }

    #[test]
    fn should_reject_an_unterminated_quote() {
        assert!(matches!(
            records("\"a,b\n"),
            Err(SimplifiImportError::UnterminatedQuote)
        ));
    }

    // --- Dates and amounts ---

    #[test]
    fn should_parse_a_two_digit_day() {
        assert_eq!(
            parse_date("Oct 27, 2026"),
            Some(date(2026, Month::October, 27))
        );
    }

    #[test]
    fn should_parse_a_one_digit_day() {
        assert_eq!(
            parse_date("Jan 5, 2026"),
            Some(date(2026, Month::January, 5))
        );
    }

    #[test]
    fn should_parse_december() {
        assert_eq!(
            parse_date("Dec 31, 2025"),
            Some(date(2025, Month::December, 31))
        );
    }

    #[test]
    fn should_reject_malformed_dates() {
        let inputs = [
            "Feb 30, 2026",
            "Foo 1, 2026",
            "oct 27, 2026",
            "2026-10-27",
            "Oct 27 2026",
            "Oct 027, 2026",
            "Oct +7, 2026",
            "Oct 27, 26",
            "Oct 27, +026",
            "",
        ];

        assert_eq!(inputs.map(parse_date), [None; 10]);
    }

    #[test]
    fn should_parse_a_signed_amount_with_leading_space() {
        assert_eq!(parse_amount(" -50000.00"), Some(-50000.0));
    }

    #[test]
    fn should_parse_a_positive_amount() {
        assert_eq!(parse_amount(" 40.25"), Some(40.25));
    }

    #[test]
    fn should_reject_non_numeric_and_non_finite_amounts() {
        assert_eq!(["abc", "", "NaN", "inf"].map(parse_amount), [None; 4]);
    }

    // --- Category mapping ---

    #[test]
    fn should_map_a_subcategory_to_its_parent() {
        assert_eq!(
            obol_category("Dining & Drinks:Fast Food", false, &accounts(&[])),
            Some("Dining & Drinks".into())
        );
    }

    #[test]
    fn should_rename_simplifi_categories_to_obol_ones() {
        let none = accounts(&[]);
        let inputs = [
            "Utilities:Phone",
            "Personal Income:Paycheck",
            "Fees & Charges",
        ];

        assert_eq!(
            inputs.map(|category| obol_category(category, false, &none)),
            [
                Some("Bills & Utilities".into()),
                Some(INCOME.into()),
                Some("Fees".into())
            ]
        );
    }

    #[test]
    fn should_keep_a_category_obol_lacks() {
        assert_eq!(
            obol_category("Charity & Donations", false, &accounts(&[])),
            Some("Charity & Donations".into())
        );
    }

    #[test]
    fn should_treat_an_excluded_transaction_as_a_transfer() {
        assert_eq!(
            obol_category("Personal Income", true, &accounts(&[])),
            Some(TRANSFER.into())
        );
    }

    #[test]
    fn should_treat_simplifi_transfer_categories_as_transfers() {
        let none = accounts(&[]);
        let inputs = ["Transfer", "Credit Card Payment", "Balance Adjustment"];

        assert_eq!(
            inputs.map(|category| obol_category(category, false, &none)),
            [
                Some(TRANSFER.into()),
                Some(TRANSFER.into()),
                Some(TRANSFER.into())
            ]
        );
    }

    #[test]
    fn should_treat_a_category_naming_an_account_as_a_transfer() {
        assert_eq!(
            obol_category("TOTAL CHECKING", false, &accounts(&["TOTAL CHECKING"])),
            Some(TRANSFER.into())
        );
    }

    #[test]
    fn should_match_an_account_name_across_non_breaking_spaces() {
        assert_eq!(
            obol_category("Apple\u{a0}Card", false, &accounts(&["Apple Card"])),
            Some(TRANSFER.into())
        );
    }

    #[test]
    fn should_give_no_category_for_uncategorized() {
        assert_eq!(obol_category("Uncategorized", false, &accounts(&[])), None);
    }

    #[test]
    fn should_give_no_category_for_an_empty_one() {
        assert_eq!(obol_category("", false, &accounts(&[])), None);
    }

    // --- Parsing an export ---

    #[test]
    fn should_parse_a_row_into_a_transaction() {
        let export =
            parsed(&["\"Oct 27, 2026\",TOTAL CHECKING,no,Corner Cafe,Dining & Drinks,no, -12.50"]);

        assert_eq!(
            export.transactions,
            [Transaction {
                source_id: "simplifi:TOTAL CHECKING".into(),
                id: "2026-10-27 -12.50 Corner Cafe #1".into(),
                posted: date(2026, Month::October, 27),
                amount: -12.5,
                description: "Corner Cafe".into(),
            }]
        );
    }

    #[test]
    fn should_normalize_non_breaking_spaces_in_the_source_id() {
        let export =
            parsed(&["\"Oct 27, 2026\",Apple\u{a0}Card,no,Corner Cafe,Shopping,no, -1.00"]);

        assert_eq!(export.transactions[0].source_id, "simplifi:Apple Card");
    }

    #[test]
    fn should_give_identical_rows_distinct_ids() {
        let row = "\"Oct 27, 2026\",Card,no,Car Wash,Auto & Transport,no, -9.00";

        let export = parsed(&[row, row]);

        assert_eq!(
            export
                .transactions
                .iter()
                .map(|t| t.id.as_str())
                .collect::<Vec<_>>(),
            [
                "2026-10-27 -9.00 Car Wash #1",
                "2026-10-27 -9.00 Car Wash #2"
            ]
        );
    }

    #[test]
    fn should_number_identical_rows_per_account() {
        let export = parsed(&[
            "\"Oct 27, 2026\",Card,no,Car Wash,Auto & Transport,no, -9.00",
            "\"Oct 27, 2026\",Checking,no,Car Wash,Auto & Transport,no, -9.00",
        ]);

        assert_eq!(export.transactions[1].id, "2026-10-27 -9.00 Car Wash #1");
    }

    #[test]
    fn should_keep_ids_stable_when_a_later_export_adds_newer_rows() {
        let old = "\"Oct 26, 2026\",Card,no,Car Wash,Auto & Transport,no, -9.00";
        let newer = "\"Oct 27, 2026\",Card,no,Car Wash,Auto & Transport,no, -9.00";

        let first = parsed(&[old]);
        let second = parsed(&[newer, old]);

        assert_eq!(second.transactions[1].id, first.transactions[0].id);
    }

    #[test]
    fn should_find_columns_by_name_in_any_order() {
        let text = "Amount,Payee,Exclusion,Category,Account,Date\n -1.00,Cafe,no,Shopping,Card,\"Oct 27, 2026\"\n";

        let export = parse_simplifi_export(text).unwrap();

        assert_eq!(export.transactions[0].amount, -1.0);
    }

    #[test]
    fn should_reject_an_export_missing_a_column() {
        let text = "Date,Account,Payee,Category,Exclusion\n";

        assert!(matches!(
            parse_simplifi_export(text),
            Err(SimplifiImportError::MissingColumn("Amount"))
        ));
    }

    #[test]
    fn should_reject_an_empty_file() {
        assert!(matches!(
            parse_simplifi_export(""),
            Err(SimplifiImportError::MissingColumn("Date"))
        ));
    }

    #[test]
    fn should_reject_a_row_with_the_wrong_number_of_fields() {
        let result = parse_simplifi_export(&export(&["\"Oct 27, 2026\",Card,no,Cafe"]));

        assert!(matches!(
            result,
            Err(SimplifiImportError::Row { row: 1, .. })
        ));
    }

    #[test]
    fn should_reject_a_row_with_a_bad_date_naming_its_row() {
        let good = "\"Oct 27, 2026\",Card,no,Cafe,Shopping,no, -1.00";
        let bad = "Someday,Card,no,Cafe,Shopping,no, -1.00";

        let result = parse_simplifi_export(&export(&[good, bad]));

        assert!(matches!(
            result,
            Err(SimplifiImportError::Row { row: 2, .. })
        ));
    }

    #[test]
    fn should_reject_a_row_with_a_bad_amount() {
        let result =
            parse_simplifi_export(&export(&["\"Oct 27, 2026\",Card,no,Cafe,Shopping,no,lots"]));

        assert!(matches!(
            result,
            Err(SimplifiImportError::Row { row: 1, .. })
        ));
    }

    #[test]
    fn should_keep_payees_out_of_row_errors() {
        let result = parse_simplifi_export(&export(&[
            "Someday,Card,no,Jane Q Private,Shopping,no, -1.00",
        ]));

        assert!(!result.unwrap_err().to_string().contains("Jane"));
    }

    // --- Seeded rules ---

    #[test]
    fn should_seed_a_rule_from_the_most_used_category() {
        let export = parsed(&[
            "\"Oct 1, 2026\",Card,no,Corner Cafe,Dining & Drinks:Coffee Shops,no, -4.00",
            "\"Oct 2, 2026\",Card,no,Corner Cafe,Dining & Drinks,no, -4.00",
            "\"Oct 3, 2026\",Card,no,Corner Cafe,Groceries,no, -4.00",
        ]);

        assert_eq!(
            export.rules.get(&merchant_key("Corner Cafe")),
            Some(&"Dining & Drinks".to_string())
        );
    }

    #[test]
    fn should_break_a_tie_alphabetically() {
        let export = parsed(&[
            "\"Oct 1, 2026\",Card,no,Corner Cafe,Groceries,no, -4.00",
            "\"Oct 2, 2026\",Card,no,Corner Cafe,Dining & Drinks,no, -4.00",
        ]);

        assert_eq!(
            export.rules.get(&merchant_key("Corner Cafe")),
            Some(&"Dining & Drinks".to_string())
        );
    }

    #[test]
    fn should_not_let_uncategorized_rows_vote() {
        let export = parsed(&[
            "\"Oct 1, 2026\",Card,no,Corner Cafe,Uncategorized,no, -4.00",
            "\"Oct 2, 2026\",Card,no,Corner Cafe,Uncategorized,no, -4.00",
            "\"Oct 3, 2026\",Card,no,Corner Cafe,Groceries,no, -4.00",
        ]);

        assert_eq!(
            export.rules.get(&merchant_key("Corner Cafe")),
            Some(&"Groceries".to_string())
        );
    }

    #[test]
    fn should_seed_no_rule_for_an_always_uncategorized_merchant() {
        let export = parsed(&["\"Oct 1, 2026\",Card,no,Mystery Shop,Uncategorized,no, -4.00"]);

        assert!(export.rules.is_empty());
    }

    #[test]
    fn should_seed_a_transfer_rule_from_an_excluded_row() {
        let export =
            parsed(&["\"Oct 1, 2026\",Checking,no,Online Move,Personal Income,yes, 500.00"]);

        assert_eq!(
            export.rules.get(&merchant_key("Online Move")),
            Some(&TRANSFER.to_string())
        );
    }

    #[test]
    fn should_seed_a_transfer_rule_when_the_category_names_another_account() {
        let export = parsed(&[
            "\"Oct 1, 2026\",Checking,no,Card Payment Out,Card,no, -100.00",
            "\"Oct 1, 2026\",Card,no,Corner Cafe,Shopping,no, -4.00",
        ]);

        assert_eq!(
            export.rules.get(&merchant_key("Card Payment Out")),
            Some(&TRANSFER.to_string())
        );
    }

    #[test]
    fn should_seed_no_rule_for_a_blank_payee() {
        let export = parsed(&["\"Oct 1, 2026\",Card,no,,Shopping,no, -4.00"]);

        assert!(export.rules.is_empty());
    }

    // --- Importing to disk ---

    fn write_export(dir: &Path, rows: &[&str]) -> PathBuf {
        let path = dir.join("export.csv");
        fs::write(&path, export(rows)).unwrap();
        path
    }

    const CAFE: &str = "\"Oct 27, 2026\",Card,no,Corner Cafe,Dining & Drinks,no, -4.00";
    const GROCER: &str = "\"Sep 3, 2026\",Card,no,Green Grocer,Groceries,no, -60.00";

    #[test]
    fn should_save_every_transaction() {
        let dir = temp_dir("save");
        let csv = write_export(&dir, &[CAFE, GROCER]);

        let summary =
            import_simplifi_export(&csv, &dir.join("transactions"), &dir.join("rules.yaml"))
                .unwrap();

        assert_eq!(
            summary.transactions,
            SaveSummary {
                added: 2,
                updated: 0
            }
        );
    }

    #[test]
    fn should_add_nothing_when_the_same_export_is_imported_again() {
        let dir = temp_dir("reimport");
        let csv = write_export(&dir, &[CAFE, GROCER]);
        let (transactions, rules) = (dir.join("transactions"), dir.join("rules.yaml"));
        import_simplifi_export(&csv, &transactions, &rules).unwrap();

        let summary = import_simplifi_export(&csv, &transactions, &rules).unwrap();

        assert_eq!(
            summary,
            SimplifiImportSummary {
                transactions: SaveSummary::default(),
                rules_added: 0
            }
        );
    }

    #[test]
    fn should_add_only_the_new_rows_of_a_later_export() {
        let dir = temp_dir("later-export");
        let (transactions, rules) = (dir.join("transactions"), dir.join("rules.yaml"));
        import_simplifi_export(&write_export(&dir, &[GROCER]), &transactions, &rules).unwrap();

        let summary =
            import_simplifi_export(&write_export(&dir, &[CAFE, GROCER]), &transactions, &rules)
                .unwrap();

        assert_eq!(
            summary.transactions,
            SaveSummary {
                added: 1,
                updated: 0
            }
        );
    }

    #[test]
    fn should_keep_an_earlier_seed_when_a_later_export_votes_otherwise() {
        let dir = temp_dir("later-seed");
        let (transactions, rules) = (dir.join("transactions"), dir.join("rules.yaml"));
        import_simplifi_export(&write_export(&dir, &[CAFE]), &transactions, &rules).unwrap();
        let regrouped = "\"Oct 28, 2026\",Card,no,Corner Cafe,Groceries,no, -4.00";

        import_simplifi_export(
            &write_export(&dir, &[regrouped, regrouped, CAFE]),
            &transactions,
            &rules,
        )
        .unwrap();

        assert_eq!(
            crate::load_or_init_merchant_rules(&rules).unwrap().rules[&merchant_key("Corner Cafe")],
            "Dining & Drinks"
        );
    }

    #[test]
    fn should_report_how_many_rules_were_seeded() {
        let dir = temp_dir("rules-added");
        let csv = write_export(&dir, &[CAFE, GROCER]);

        let summary =
            import_simplifi_export(&csv, &dir.join("transactions"), &dir.join("rules.yaml"))
                .unwrap();

        assert_eq!(summary.rules_added, 2);
    }

    #[test]
    fn should_keep_an_existing_rule_over_a_seeded_one() {
        let dir = temp_dir("keep-rule");
        let csv = write_export(&dir, &[CAFE]);
        let rules = dir.join("rules.yaml");
        crate::set_merchant_category(&rules, &merchant_key("Corner Cafe"), "Groceries").unwrap();

        import_simplifi_export(&csv, &dir.join("transactions"), &rules).unwrap();

        assert_eq!(
            crate::load_or_init_merchant_rules(&rules).unwrap().rules[&merchant_key("Corner Cafe")],
            "Groceries"
        );
    }

    #[test]
    fn should_save_nothing_when_a_row_is_bad() {
        let dir = temp_dir("all-or-nothing");
        let csv = write_export(&dir, &[CAFE, "Someday,Card,no,Cafe,Shopping,no, -1.00"]);
        let transactions = dir.join("transactions");

        let _ = import_simplifi_export(&csv, &transactions, &dir.join("rules.yaml"));

        assert!(!transactions.exists());
    }

    #[test]
    fn should_fail_when_the_export_is_missing() {
        let dir = temp_dir("missing");

        let result = import_simplifi_export(
            &dir.join("nope.csv"),
            &dir.join("transactions"),
            &dir.join("rules.yaml"),
        );

        assert!(matches!(result, Err(SimplifiImportError::Io(_))));
    }
}
