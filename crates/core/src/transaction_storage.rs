//! Transaction storage (spec §11.4, D44): one JSON file per calendar
//! month, `<dir>/YYYY-MM.json`, written through `atomic_write` (`0600`
//! files, `0700` directory).
//!
//! **Contract (task 111b), implemented by task 112.** Upsert is keyed on
//! (`source_id`, `id`): saving a transaction that's already stored replaces
//! it rather than duplicating it. Loads return newest first, ties broken by
//! `source_id` then `id`, so the order is stable.

use std::path::Path;

use thiserror::Error;

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
#[allow(unused_variables)] // stub until task 112
pub fn save_transactions(
    dir: &Path,
    transactions: &[Transaction],
) -> Result<SaveSummary, TransactionStorageError> {
    todo!("task 112")
}

/// Every transaction posted in `month`, newest first; empty if the month
/// has no file.
#[allow(unused_variables)] // stub until task 112
pub fn load_transactions(
    dir: &Path,
    month: YearMonth,
) -> Result<Vec<Transaction>, TransactionStorageError> {
    todo!("task 112")
}

/// Every transaction posted from `from` through `to` inclusive, newest
/// first. An inverted range is empty.
#[allow(unused_variables)] // stub until task 112
pub fn load_transactions_range(
    dir: &Path,
    from: YearMonth,
    to: YearMonth,
) -> Result<Vec<Transaction>, TransactionStorageError> {
    todo!("task 112")
}
