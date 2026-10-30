//! Transaction storage (spec §11.4, D44): one JSON file per calendar
//! month, `<dir>/YYYY-MM.json`, written through `atomic_write` (`0600`
//! files, `0700` directory).
//!
//! **Contract (task 111b), implemented by task 112.** Upsert is keyed on
//! (`source_id`, `id`): saving a transaction that's already stored replaces
//! it rather than duplicating it. Loads return newest first, ties broken by
//! `source_id` then `id`, so the order is stable.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::atomic_write::write_atomically;
use crate::transaction::{Transaction, YearMonth};

#[derive(Debug, Error)]
pub enum TransactionStorageError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("a transactions file could not be parsed: {0}")]
    Parse(#[from] serde_json::Error),
}

/// How a save changed the store.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SaveSummary {
    pub added: usize,
    pub updated: usize,
}

/// Upserts each transaction into the file for the month it posted in.
/// `dir` is the transactions directory (`StorageLayout::transactions`).
pub fn save_transactions(
    dir: &Path,
    transactions: &[Transaction],
) -> Result<SaveSummary, TransactionStorageError> {
    let mut by_month: BTreeMap<YearMonth, Vec<&Transaction>> = BTreeMap::new();
    for transaction in transactions {
        by_month
            .entry(YearMonth::of(transaction.posted))
            .or_default()
            .push(transaction);
    }

    let mut summary = SaveSummary::default();
    for (month, incoming) in by_month {
        let path = month_path(dir, month);
        // A corrupt file fails here, before anything could overwrite it.
        let mut stored = read_month(&path)?;
        let before = summary;
        // Keys are only looked up within the posting month: a posted
        // transaction's date doesn't change, so it can't be in another file.
        for transaction in incoming {
            let existing = stored
                .iter_mut()
                .find(|s| s.source_id == transaction.source_id && s.id == transaction.id);
            match existing {
                Some(existing) if existing != transaction => {
                    *existing = transaction.clone();
                    summary.updated += 1;
                }
                Some(_) => {}
                None => {
                    stored.push(transaction.clone());
                    summary.added += 1;
                }
            }
        }
        if summary != before {
            write_atomically(&path, &serde_json::to_vec_pretty(&stored)?)?;
        }
    }
    Ok(summary)
}

/// Every transaction posted in `month`, newest first; empty if the month
/// has no file.
pub fn load_transactions(
    dir: &Path,
    month: YearMonth,
) -> Result<Vec<Transaction>, TransactionStorageError> {
    load_transactions_range(dir, month, month)
}

/// Every transaction posted from `from` through `to` inclusive, newest
/// first. An inverted range is empty.
pub fn load_transactions_range(
    dir: &Path,
    from: YearMonth,
    to: YearMonth,
) -> Result<Vec<Transaction>, TransactionStorageError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };

    let mut transactions = Vec::new();
    for entry in entries {
        let name = entry?.file_name();
        let month = name
            .to_str()
            .and_then(|name| name.strip_suffix(".json"))
            .and_then(YearMonth::parse);
        if let Some(month) = month.filter(|month| (from..=to).contains(month)) {
            // Unlike snapshots, a corrupt month is an error, not skipped:
            // dropping it would understate that month's spending.
            transactions.extend(read_month(&month_path(dir, month))?);
        }
    }
    transactions.sort_by(|a, b| {
        b.posted
            .cmp(&a.posted)
            .then_with(|| a.source_id.cmp(&b.source_id))
            .then_with(|| a.id.cmp(&b.id))
    });
    Ok(transactions)
}

fn month_path(dir: &Path, month: YearMonth) -> PathBuf {
    dir.join(format!("{month}.json"))
}

/// A month's stored transactions; empty if it has no file yet.
fn read_month(path: &Path) -> Result<Vec<Transaction>, TransactionStorageError> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use time::{Date, Month};

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "obol-transaction-storage-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        dir
    }

    fn ym(year: i32, month: u8) -> YearMonth {
        YearMonth::new(year, month).unwrap()
    }

    fn txn(
        source_id: &str,
        id: &str,
        (year, month, day): (i32, u8, u8),
        amount: f64,
    ) -> Transaction {
        Transaction {
            source_id: source_id.into(),
            id: id.into(),
            posted: Date::from_calendar_date(year, Month::try_from(month).unwrap(), day).unwrap(),
            amount,
            description: "AMAZON MKTPL*AB12CD".into(),
        }
    }

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn should_round_trip_saved_transactions() {
        let dir = temp_dir("round-trip");
        let saved = txn("chase", "t1", (2026, 10, 26), -22.18);
        save_transactions(&dir, std::slice::from_ref(&saved)).unwrap();

        let loaded = load_transactions(&dir, ym(2026, 10)).unwrap();

        assert_eq!(loaded, [saved]);
    }

    #[test]
    fn should_load_empty_when_the_directory_is_missing() {
        let dir = temp_dir("missing-dir");

        let loaded = load_transactions(&dir, ym(2026, 10)).unwrap();

        assert_eq!(loaded, []);
    }

    #[test]
    fn should_load_empty_when_the_month_has_no_file() {
        let dir = temp_dir("missing-month");
        save_transactions(&dir, &[txn("chase", "t1", (2026, 9, 1), -1.0)]).unwrap();

        let loaded = load_transactions(&dir, ym(2026, 10)).unwrap();

        assert_eq!(loaded, []);
    }

    #[test]
    fn should_replace_rather_than_duplicate_a_resaved_key() {
        let dir = temp_dir("upsert");
        save_transactions(&dir, &[txn("chase", "t1", (2026, 10, 1), -5.0)]).unwrap();
        let replacement = txn("chase", "t1", (2026, 10, 1), -6.0);

        save_transactions(&dir, std::slice::from_ref(&replacement)).unwrap();

        assert_eq!(
            load_transactions(&dir, ym(2026, 10)).unwrap(),
            [replacement]
        );
    }

    #[test]
    fn should_keep_the_same_id_from_different_sources_apart() {
        let dir = temp_dir("same-id");
        save_transactions(&dir, &[txn("amex", "t1", (2026, 10, 1), -5.0)]).unwrap();

        save_transactions(&dir, &[txn("chase", "t1", (2026, 10, 1), -5.0)]).unwrap();

        assert_eq!(load_transactions(&dir, ym(2026, 10)).unwrap().len(), 2);
    }

    #[test]
    fn should_keep_different_ids_from_the_same_source_apart() {
        let dir = temp_dir("same-source");
        save_transactions(&dir, &[txn("chase", "t1", (2026, 10, 1), -5.0)]).unwrap();

        save_transactions(&dir, &[txn("chase", "t2", (2026, 10, 1), -5.0)]).unwrap();

        assert_eq!(load_transactions(&dir, ym(2026, 10)).unwrap().len(), 2);
    }

    #[test]
    fn should_count_new_keys_as_added() {
        let dir = temp_dir("added");

        let summary = save_transactions(
            &dir,
            &[
                txn("chase", "t1", (2026, 10, 1), -5.0),
                txn("chase", "t2", (2026, 11, 1), -5.0),
            ],
        )
        .unwrap();

        assert_eq!(
            summary,
            SaveSummary {
                added: 2,
                updated: 0
            }
        );
    }

    #[test]
    fn should_count_changed_keys_as_updated() {
        let dir = temp_dir("updated");
        save_transactions(&dir, &[txn("chase", "t1", (2026, 10, 1), -5.0)]).unwrap();

        let summary = save_transactions(
            &dir,
            &[
                txn("chase", "t1", (2026, 10, 1), -6.0),
                txn("chase", "t2", (2026, 10, 2), -5.0),
            ],
        )
        .unwrap();

        assert_eq!(
            summary,
            SaveSummary {
                added: 1,
                updated: 1
            }
        );
    }

    #[test]
    fn should_count_an_identical_resave_as_neither() {
        let dir = temp_dir("identical");
        let saved = txn("chase", "t1", (2026, 10, 1), -5.0);
        save_transactions(&dir, std::slice::from_ref(&saved)).unwrap();

        let summary = save_transactions(&dir, &[saved]).unwrap();

        assert_eq!(summary, SaveSummary::default());
    }

    #[test]
    fn should_not_rewrite_a_month_whose_contents_are_unchanged() {
        // A compact file the store would never write itself shows whether
        // the re-save touched it.
        let dir = temp_dir("no-rewrite");
        let saved = txn("chase", "t1", (2026, 10, 1), -5.0);
        save_transactions(&dir, std::slice::from_ref(&saved)).unwrap();
        let path = dir.join("2026-10.json");
        let compact = serde_json::to_vec(&[&saved]).unwrap();
        fs::write(&path, &compact).unwrap();

        save_transactions(&dir, &[saved]).unwrap();

        assert_eq!(fs::read(&path).unwrap(), compact);
    }

    #[test]
    fn should_store_each_transaction_in_the_file_for_its_posting_month() {
        let dir = temp_dir("split");

        save_transactions(
            &dir,
            &[
                txn("chase", "t1", (2026, 10, 31), -5.0),
                txn("chase", "t2", (2026, 11, 1), -5.0),
            ],
        )
        .unwrap();

        assert_eq!(file_names(&dir), ["2026-10.json", "2026-11.json"]);
    }

    #[test]
    fn should_load_only_the_requested_month() {
        let dir = temp_dir("one-month");
        let october = txn("chase", "t1", (2026, 10, 31), -5.0);
        save_transactions(
            &dir,
            &[october.clone(), txn("chase", "t2", (2026, 11, 1), -5.0)],
        )
        .unwrap();

        assert_eq!(load_transactions(&dir, ym(2026, 10)).unwrap(), [october]);
    }

    #[test]
    fn should_load_newest_first() {
        let dir = temp_dir("newest-first");
        let older = txn("chase", "t1", (2026, 10, 1), -5.0);
        let newer = txn("chase", "t2", (2026, 10, 2), -5.0);
        save_transactions(&dir, &[older.clone(), newer.clone()]).unwrap();

        assert_eq!(
            load_transactions(&dir, ym(2026, 10)).unwrap(),
            [newer, older]
        );
    }

    #[test]
    fn should_break_date_ties_by_source_then_id() {
        let dir = temp_dir("tie-break");
        let (a1, a2, b1) = (
            txn("amex", "t1", (2026, 10, 1), -5.0),
            txn("amex", "t2", (2026, 10, 1), -5.0),
            txn("chase", "t1", (2026, 10, 1), -5.0),
        );
        save_transactions(&dir, &[b1.clone(), a2.clone(), a1.clone()]).unwrap();

        assert_eq!(load_transactions(&dir, ym(2026, 10)).unwrap(), [a1, a2, b1]);
    }

    #[test]
    fn should_load_a_range_inclusive_of_both_ends() {
        let dir = temp_dir("range");
        let (sep, oct, nov, dec) = (
            txn("chase", "t1", (2026, 9, 15), -5.0),
            txn("chase", "t2", (2026, 10, 15), -5.0),
            txn("chase", "t3", (2026, 11, 15), -5.0),
            txn("chase", "t4", (2026, 12, 15), -5.0),
        );
        save_transactions(&dir, &[sep, oct.clone(), nov.clone(), dec]).unwrap();

        let loaded = load_transactions_range(&dir, ym(2026, 10), ym(2026, 11)).unwrap();

        assert_eq!(loaded, [nov, oct]);
    }

    #[test]
    fn should_load_empty_for_an_inverted_range() {
        let dir = temp_dir("inverted");
        save_transactions(
            &dir,
            &[
                txn("chase", "t1", (2026, 10, 15), -5.0),
                txn("chase", "t2", (2026, 11, 15), -5.0),
            ],
        )
        .unwrap();

        let loaded = load_transactions_range(&dir, ym(2026, 11), ym(2026, 10)).unwrap();

        assert_eq!(loaded, []);
    }

    #[test]
    fn should_load_a_range_across_a_year_boundary_newest_first() {
        let dir = temp_dir("year-boundary");
        let december = txn("chase", "t1", (2025, 12, 31), -5.0);
        let january = txn("chase", "t2", (2026, 1, 1), -5.0);
        save_transactions(&dir, &[december.clone(), january.clone()]).unwrap();

        let loaded = load_transactions_range(&dir, ym(2025, 12), ym(2026, 1)).unwrap();

        assert_eq!(loaded, [january, december]);
    }

    #[test]
    fn should_load_an_empty_range_when_the_directory_is_missing() {
        let dir = temp_dir("range-missing-dir");

        let loaded = load_transactions_range(&dir, ym(2026, 1), ym(2026, 12)).unwrap();

        assert_eq!(loaded, []);
    }

    #[test]
    fn should_ignore_files_that_are_not_month_files() {
        let dir = temp_dir("stray-files");
        let saved = txn("chase", "t1", (2026, 10, 15), -5.0);
        save_transactions(&dir, std::slice::from_ref(&saved)).unwrap();
        fs::write(dir.join("2026-10.json.tmp"), "not json").unwrap();
        fs::write(dir.join("2026-11.txt"), "not json").unwrap();
        fs::write(dir.join("notes.json"), "not json").unwrap();

        let loaded = load_transactions_range(&dir, ym(2026, 1), ym(2026, 12)).unwrap();

        assert_eq!(loaded, [saved]);
    }

    #[test]
    fn should_fail_to_load_a_corrupt_month_file() {
        // Skipping it would silently understate that month's spending.
        let dir = temp_dir("corrupt-load");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("2026-10.json"), "{ not json").unwrap();

        let result = load_transactions(&dir, ym(2026, 10));

        assert!(matches!(result, Err(TransactionStorageError::Parse(_))));
    }

    #[test]
    fn should_fail_a_range_load_over_a_corrupt_month_file() {
        let dir = temp_dir("corrupt-range");
        save_transactions(&dir, &[txn("chase", "t1", (2026, 9, 15), -5.0)]).unwrap();
        fs::write(dir.join("2026-10.json"), "{ not json").unwrap();

        let result = load_transactions_range(&dir, ym(2026, 9), ym(2026, 10));

        assert!(matches!(result, Err(TransactionStorageError::Parse(_))));
    }

    #[test]
    fn should_refuse_to_save_into_a_corrupt_month_file() {
        let dir = temp_dir("corrupt-save");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("2026-10.json"), "{ not json").unwrap();

        let result = save_transactions(&dir, &[txn("chase", "t1", (2026, 10, 15), -5.0)]);

        assert!(matches!(result, Err(TransactionStorageError::Parse(_))));
    }

    #[test]
    fn should_leave_a_corrupt_month_file_untouched_on_save() {
        let dir = temp_dir("corrupt-untouched");
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("2026-10.json");
        fs::write(&path, "{ not json").unwrap();

        let _ = save_transactions(&dir, &[txn("chase", "t1", (2026, 10, 15), -5.0)]);

        assert_eq!(fs::read_to_string(&path).unwrap(), "{ not json");
    }

    #[test]
    fn should_write_month_files_with_0600_permissions() {
        let dir = temp_dir("perms");
        save_transactions(&dir, &[txn("chase", "t1", (2026, 10, 15), -5.0)]).unwrap();

        let mode = fs::metadata(dir.join("2026-10.json"))
            .unwrap()
            .permissions()
            .mode();

        assert_eq!(mode & 0o777, 0o600);
    }
}
