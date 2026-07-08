//! Tracks which statement PDFs have already been parsed, per source
//! (spec §6.3, D28) — so `StatementImportProvider::fetch()` doesn't
//! re-parse a statement it's already seen, and can still report a
//! balance on runs where no new statement has shown up (a dropbox only
//! gets a new file monthly; without this, every run between statements
//! would have nothing to report). Scoped here to the ledger's data model
//! — persistence is `statement_import_storage.rs`'s concern, mirroring
//! `item_usage.rs`/`item_usage_storage.rs`'s split.

use std::collections::HashMap;

use serde::{Deserialize, Serialize};

/// One source's processed-file history plus its last known balance, so a
/// run with no new statement can still report the same balance again
/// rather than showing no data.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
struct SourceLedgerEntry {
    /// Filename -> sha256 of that file's contents at the time it was
    /// processed, so a same-named file with different contents (e.g. a
    /// corrected re-download) is treated as new.
    processed_filenames: HashMap<String, String>,
    last_balance: f64,
    last_as_of_date: String,
    /// The raw account identifier the parser extracted from the most
    /// recently processed statement (e.g. a last-4) — retained
    /// alongside the balance so a "no new statement" run hashes the
    /// exact same `account_key` a fresh parse would have produced,
    /// rather than a different fallback value. Account-key stability
    /// across runs (D15) must not depend on whether this run happened
    /// to find a new file.
    last_account_identifier: String,
}

/// Per-source processed-file tracking (spec §6.3). Keyed by
/// `SourceConfig.id`, since each source has its own `watch_dir` and
/// shouldn't need to re-scan every other source's state.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ProcessedFilesLedger {
    per_source: HashMap<String, SourceLedgerEntry>,
}

impl ProcessedFilesLedger {
    pub fn new() -> Self {
        Self::default()
    }

    /// True if this exact file (by content hash, not just filename) has
    /// already been successfully parsed for this source.
    pub fn is_processed(&self, source_id: &str, filename: &str, content_hash: &str) -> bool {
        self.per_source
            .get(source_id)
            .and_then(|entry| entry.processed_filenames.get(filename))
            .is_some_and(|hash| hash == content_hash)
    }

    /// Records a filename+hash as processed and updates the source's
    /// last-known balance/identifier. Call only after a successful
    /// parse — a parse failure must never be recorded here, so the same
    /// file is retried next run once it's fixed, instead of being
    /// silently skipped forever.
    pub fn mark_processed(
        &mut self,
        source_id: &str,
        filename: &str,
        content_hash: &str,
        balance: f64,
        as_of_date: &str,
        account_identifier: &str,
    ) {
        let entry = self.per_source.entry(source_id.to_string()).or_default();
        entry
            .processed_filenames
            .insert(filename.to_string(), content_hash.to_string());
        entry.last_balance = balance;
        entry.last_as_of_date = as_of_date.to_string();
        entry.last_account_identifier = account_identifier.to_string();
    }

    /// The balance/as-of-date/account-identifier from the most recently
    /// processed statement for this source, if any has ever been
    /// processed — the fallback used when a run finds no new statement
    /// to parse. Returning the same `account_identifier` here as the
    /// original parse produced is what keeps `account_key` stable
    /// across runs regardless of whether this particular run found a
    /// new file.
    pub fn last_known(&self, source_id: &str) -> Option<(f64, &str, &str)> {
        self.per_source.get(source_id).map(|entry| {
            (
                entry.last_balance,
                entry.last_as_of_date.as_str(),
                entry.last_account_identifier.as_str(),
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_ledger_has_no_processed_files_or_last_known_balance() {
        let ledger = ProcessedFilesLedger::new();
        assert!(!ledger.is_processed("chase_checking", "statement.pdf", "abc123"));
        assert_eq!(ledger.last_known("chase_checking"), None);
    }

    #[test]
    fn mark_processed_makes_is_processed_true_for_the_same_hash() {
        let mut ledger = ProcessedFilesLedger::new();
        ledger.mark_processed(
            "chase_checking",
            "statement.pdf",
            "abc123",
            100.0,
            "2026-06-30",
            "6789",
        );

        assert!(ledger.is_processed("chase_checking", "statement.pdf", "abc123"));
    }

    #[test]
    fn a_same_named_file_with_different_content_is_not_considered_processed() {
        let mut ledger = ProcessedFilesLedger::new();
        ledger.mark_processed(
            "chase_checking",
            "statement.pdf",
            "abc123",
            100.0,
            "2026-06-30",
            "6789",
        );

        // Same filename, different content hash — e.g. a corrected
        // re-download replacing the original.
        assert!(!ledger.is_processed("chase_checking", "statement.pdf", "different-hash"));
    }

    #[test]
    fn last_known_reflects_the_most_recently_marked_file() {
        let mut ledger = ProcessedFilesLedger::new();
        ledger.mark_processed(
            "chase_checking",
            "june.pdf",
            "hash-june",
            100.0,
            "2026-06-30",
            "6789",
        );
        ledger.mark_processed(
            "chase_checking",
            "july.pdf",
            "hash-july",
            150.0,
            "2026-07-31",
            "6789",
        );

        assert_eq!(
            ledger.last_known("chase_checking"),
            Some((150.0, "2026-07-31", "6789"))
        );
    }

    #[test]
    fn last_known_account_identifier_stays_the_same_across_a_no_new_file_run() {
        // This is the property that keeps account_key stable (D15)
        // regardless of whether a given run finds a new statement:
        // last_known() must hand back the exact identifier the original
        // parse produced, not something the caller has to reconstruct
        // differently on a "nothing new" run.
        let mut ledger = ProcessedFilesLedger::new();
        ledger.mark_processed(
            "chase_checking",
            "statement.pdf",
            "abc123",
            100.0,
            "2026-06-30",
            "6789",
        );

        let (_, _, identifier) = ledger.last_known("chase_checking").unwrap();
        assert_eq!(identifier, "6789");
    }

    #[test]
    fn each_source_is_tracked_independently() {
        let mut ledger = ProcessedFilesLedger::new();
        ledger.mark_processed(
            "chase_checking",
            "statement.pdf",
            "hash-a",
            100.0,
            "2026-06-30",
            "6789",
        );

        // A different source's ledger entry is untouched.
        assert!(!ledger.is_processed("vanguard_brokerage", "statement.pdf", "hash-a"));
        assert_eq!(ledger.last_known("vanguard_brokerage"), None);
        assert_eq!(
            ledger.last_known("chase_checking"),
            Some((100.0, "2026-06-30", "6789"))
        );
    }

    #[test]
    fn serializes_and_deserializes_correctly() {
        let mut ledger = ProcessedFilesLedger::new();
        ledger.mark_processed(
            "chase_checking",
            "statement.pdf",
            "abc123",
            100.0,
            "2026-06-30",
            "6789",
        );

        let json = serde_json::to_string(&ledger).unwrap();
        let round_tripped: ProcessedFilesLedger = serde_json::from_str(&json).unwrap();

        assert_eq!(round_tripped, ledger);
    }
}
