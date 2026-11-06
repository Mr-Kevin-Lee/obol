//! Learning categories from a Simplifi export (spec §11.4, D44): seeds
//! `merchant_rules.yaml` with the category Simplifi used most for each
//! merchant. Only categories: SimpleFIN supplies the transactions, so
//! importing Simplifi's too would count every purchase twice. Local only;
//! an export is never committed and its payees never reach a log line or an
//! error message.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use thiserror::Error;

use crate::categorize::{INCOME, TRANSFER, UNCATEGORIZED};
use crate::merchant::merchant_key;
use crate::merchant_rules_storage::{seed_merchant_rules, MerchantRulesError};

#[derive(Debug, Error)]
pub enum SimplifiImportError {
    #[error("not a Simplifi transactions export: it has no {0:?} column")]
    MissingColumn(&'static str),
    #[error("the export ends inside a quoted field")]
    UnterminatedQuote,
    /// `row` counts data rows from 1, after the header.
    #[error("row {row}: {problem}")]
    Row { row: usize, problem: String },
    #[error(transparent)]
    MerchantRules(#[from] MerchantRulesError),
}

/// Simplifi's own names for a move between accounts; it also uses an
/// account's name as the category of a transfer into it.
const TRANSFER_CATEGORIES: &[&str] = &["Transfer", "Credit Card Payment", "Balance Adjustment"];

/// Merchant key → the category Simplifi used most for it. Only the
/// `Account`, `Payee`, `Category` and `Exclusion` columns are read.
pub fn parse_simplifi_rules(text: &str) -> Result<BTreeMap<String, String>, SimplifiImportError> {
    let mut records = records(text)?.into_iter();
    let header = records.next().unwrap_or_default();
    let column = |name: &'static str| {
        header
            .iter()
            .position(|field| field == name)
            .ok_or(SimplifiImportError::MissingColumn(name))
    };
    let (account, payee, category, exclusion) = (
        column("Account")?,
        column("Payee")?,
        column("Category")?,
        column("Exclusion")?,
    );
    let rows: Vec<Vec<String>> = records.collect();
    let accounts: BTreeSet<String> = rows
        .iter()
        .filter_map(|row| row.get(account))
        .map(|name| collapse_spaces(name))
        .collect();

    let mut votes: BTreeMap<String, BTreeMap<String, usize>> = BTreeMap::new();
    for (index, row) in rows.iter().enumerate() {
        // Names the row, never its payee.
        if row.len() != header.len() {
            return Err(SimplifiImportError::Row {
                row: index + 1,
                problem: format!("{} fields, expected {}", row.len(), header.len()),
            });
        }
        let key = merchant_key(&row[payee]);
        if let Some(category) = obol_category(&row[category], row[exclusion] == "yes", &accounts) {
            if !key.is_empty() {
                *votes.entry(key).or_default().entry(category).or_default() += 1;
            }
        }
    }

    Ok(votes
        .into_iter()
        .filter_map(|(key, counts)| {
            // Most votes wins; a tie goes to the alphabetically first.
            let (category, _) = counts
                .into_iter()
                .max_by(|a, b| a.1.cmp(&b.1).then_with(|| b.0.cmp(&a.0)))?;
            Some((key, category))
        })
        .collect())
}

/// Learns the categories in `export`, the file's text: the app reads the
/// file the user picks, so this never opens one itself. Additive: a merchant
/// that already has a rule — an earlier import or an edit in the app —
/// keeps it. A malformed row fails the whole import before anything is
/// written. Returns how many rules were added.
pub fn import_simplifi_rules(
    export: &str,
    merchant_rules: &Path,
) -> Result<usize, SimplifiImportError> {
    Ok(seed_merchant_rules(
        merchant_rules,
        &parse_simplifi_rules(export)?,
    )?)
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
    use std::fs;
    use std::path::PathBuf;

    const HEADER: &str = "Date,Account,Reviewed,Payee,Category,Exclusion,Amount";

    fn export(rows: &[&str]) -> String {
        std::iter::once(HEADER)
            .chain(rows.iter().copied())
            .map(|line| format!("{line}\n"))
            .collect()
    }

    fn parsed(rows: &[&str]) -> BTreeMap<String, String> {
        parse_simplifi_rules(&export(rows)).unwrap()
    }

    fn accounts(names: &[&str]) -> BTreeSet<String> {
        names.iter().map(|name| name.to_string()).collect()
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

    // --- Learning rules from an export ---

    #[test]
    fn should_learn_the_most_used_category() {
        let rules = parsed(&[
            "\"Oct 1, 2026\",Card,no,Corner Cafe,Dining & Drinks:Coffee Shops,no, -4.00",
            "\"Oct 2, 2026\",Card,no,Corner Cafe,Dining & Drinks,no, -4.00",
            "\"Oct 3, 2026\",Card,no,Corner Cafe,Groceries,no, -4.00",
        ]);

        assert_eq!(
            rules.get(&merchant_key("Corner Cafe")),
            Some(&"Dining & Drinks".to_string())
        );
    }

    #[test]
    fn should_break_a_tie_alphabetically() {
        let rules = parsed(&[
            "\"Oct 1, 2026\",Card,no,Corner Cafe,Groceries,no, -4.00",
            "\"Oct 2, 2026\",Card,no,Corner Cafe,Dining & Drinks,no, -4.00",
        ]);

        assert_eq!(
            rules.get(&merchant_key("Corner Cafe")),
            Some(&"Dining & Drinks".to_string())
        );
    }

    #[test]
    fn should_not_let_uncategorized_rows_vote() {
        let rules = parsed(&[
            "\"Oct 1, 2026\",Card,no,Corner Cafe,Uncategorized,no, -4.00",
            "\"Oct 2, 2026\",Card,no,Corner Cafe,Uncategorized,no, -4.00",
            "\"Oct 3, 2026\",Card,no,Corner Cafe,Groceries,no, -4.00",
        ]);

        assert_eq!(
            rules.get(&merchant_key("Corner Cafe")),
            Some(&"Groceries".to_string())
        );
    }

    #[test]
    fn should_learn_nothing_for_an_always_uncategorized_merchant() {
        let rules = parsed(&["\"Oct 1, 2026\",Card,no,Mystery Shop,Uncategorized,no, -4.00"]);

        assert!(rules.is_empty());
    }

    #[test]
    fn should_learn_a_transfer_from_an_excluded_row() {
        let rules =
            parsed(&["\"Oct 1, 2026\",Checking,no,Online Move,Personal Income,yes, 500.00"]);

        assert_eq!(
            rules.get(&merchant_key("Online Move")),
            Some(&TRANSFER.to_string())
        );
    }

    #[test]
    fn should_learn_a_transfer_when_the_category_names_another_account() {
        let rules = parsed(&[
            "\"Oct 1, 2026\",Checking,no,Card Payment Out,Card,no, -100.00",
            "\"Oct 1, 2026\",Card,no,Corner Cafe,Shopping,no, -4.00",
        ]);

        assert_eq!(
            rules.get(&merchant_key("Card Payment Out")),
            Some(&TRANSFER.to_string())
        );
    }

    #[test]
    fn should_recognize_an_account_written_with_non_breaking_spaces() {
        let rules = parsed(&[
            "\"Oct 1, 2026\",Checking,no,Card Payment Out,Apple Card,no, -100.00",
            "\"Oct 1, 2026\",Apple\u{a0}Card,no,Corner Cafe,Shopping,no, -4.00",
        ]);

        assert_eq!(
            rules.get(&merchant_key("Card Payment Out")),
            Some(&TRANSFER.to_string())
        );
    }

    #[test]
    fn should_learn_nothing_for_a_blank_payee() {
        let rules = parsed(&["\"Oct 1, 2026\",Card,no,,Shopping,no, -4.00"]);

        assert!(rules.is_empty());
    }

    #[test]
    fn should_find_columns_by_name_in_any_order() {
        let text = "Exclusion,Category,Payee,Account\nno,Shopping,Corner Cafe,Card\n";

        let rules = parse_simplifi_rules(text).unwrap();

        assert_eq!(
            rules.get(&merchant_key("Corner Cafe")),
            Some(&"Shopping".to_string())
        );
    }

    #[test]
    fn should_not_need_date_or_amount_columns() {
        let text = "Account,Payee,Category,Exclusion\nCard,Corner Cafe,Shopping,no\n";

        assert!(parse_simplifi_rules(text).is_ok());
    }

    #[test]
    fn should_reject_an_export_missing_a_column() {
        let text = "Date,Account,Payee,Exclusion,Amount\n";

        assert!(matches!(
            parse_simplifi_rules(text),
            Err(SimplifiImportError::MissingColumn("Category"))
        ));
    }

    #[test]
    fn should_reject_an_empty_file() {
        assert!(matches!(
            parse_simplifi_rules(""),
            Err(SimplifiImportError::MissingColumn("Account"))
        ));
    }

    #[test]
    fn should_reject_a_row_with_the_wrong_number_of_fields_naming_its_row() {
        let good = "\"Oct 27, 2026\",Card,no,Cafe,Shopping,no, -1.00";

        let result = parse_simplifi_rules(&export(&[good, "\"Oct 27, 2026\",Card,no,Cafe"]));

        assert!(matches!(
            result,
            Err(SimplifiImportError::Row { row: 2, .. })
        ));
    }

    #[test]
    fn should_keep_payees_out_of_row_errors() {
        let result = parse_simplifi_rules(&export(&["\"Oct 27, 2026\",Card,no,Jane Q Private"]));

        assert!(!result.unwrap_err().to_string().contains("Jane"));
    }

    // --- Saving what was learned ---

    const CAFE: &str = "\"Oct 27, 2026\",Card,no,Corner Cafe,Dining & Drinks,no, -4.00";
    const GROCER: &str = "\"Sep 3, 2026\",Card,no,Green Grocer,Groceries,no, -60.00";

    #[test]
    fn should_report_how_many_rules_were_learned() {
        let dir = temp_dir("rules-added");

        let added =
            import_simplifi_rules(&export(&[CAFE, GROCER]), &dir.join("rules.yaml")).unwrap();

        assert_eq!(added, 2);
    }

    #[test]
    fn should_learn_nothing_new_from_the_same_export_twice() {
        let dir = temp_dir("reimport");
        let rules = dir.join("rules.yaml");
        import_simplifi_rules(&export(&[CAFE, GROCER]), &rules).unwrap();

        assert_eq!(
            import_simplifi_rules(&export(&[CAFE, GROCER]), &rules).unwrap(),
            0
        );
    }

    #[test]
    fn should_keep_an_existing_rule_over_a_learned_one() {
        let dir = temp_dir("keep-rule");
        let rules = dir.join("rules.yaml");
        crate::set_merchant_category(&rules, &merchant_key("Corner Cafe"), "Groceries").unwrap();

        import_simplifi_rules(&export(&[CAFE]), &rules).unwrap();

        assert_eq!(
            crate::load_or_init_merchant_rules(&rules).unwrap().rules[&merchant_key("Corner Cafe")],
            "Groceries"
        );
    }

    #[test]
    fn should_write_nothing_when_a_row_is_bad() {
        let dir = temp_dir("all-or-nothing");
        let rules = dir.join("rules.yaml");

        let _ = import_simplifi_rules(&export(&[CAFE, "\"Oct 27, 2026\",Card,no"]), &rules);

        assert!(!rules.exists());
    }
}
