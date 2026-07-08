//! Persistence for the statement-import processed-files ledger (spec
//! §6.3, D28). `statement_import/processed_files.rs` owns the ledger's
//! data model; this is the storage-layer wiring, mirroring
//! `item_usage.rs`/`item_usage_storage.rs`'s split. Same atomic-write +
//! `0600` pattern as `sources.rs`/`item_usage_storage.rs`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use thiserror::Error;

use crate::statement_import::ProcessedFilesLedger;

#[derive(Debug, Error)]
pub enum ProcessedFilesStorageError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("processed-files ledger could not be parsed: {0}")]
    Parse(serde_json::Error),
}

/// Loads the ledger, creating a fresh (empty) one on disk if it doesn't
/// exist yet — mirrors `load_or_init_item_usage`'s first-run behavior.
pub fn load_or_init_processed_files(
    path: &Path,
) -> Result<ProcessedFilesLedger, ProcessedFilesStorageError> {
    if !path.exists() {
        let ledger = ProcessedFilesLedger::new();
        save_processed_files(path, &ledger)?;
        return Ok(ledger);
    }

    let contents = fs::read_to_string(path)?;
    serde_json::from_str(&contents).map_err(ProcessedFilesStorageError::Parse)
}

/// Atomic write (temp file + rename) with `0600` permissions (§4) — same
/// protection as every other file under the storage directory.
pub fn save_processed_files(
    path: &Path,
    ledger: &ProcessedFilesLedger,
) -> Result<(), ProcessedFilesStorageError> {
    let json = serde_json::to_string_pretty(ledger)
        .expect("ProcessedFilesLedger serialization cannot fail");

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }

    let temp_path = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(json.as_bytes())?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temp_path, path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "obol-processed-files-storage-test-{name}-{}.json",
            std::process::id()
        ))
    }

    #[test]
    fn load_or_init_creates_an_empty_ledger_on_first_run() {
        let path = temp_path("first-run");
        let _ = fs::remove_file(&path);

        let ledger = load_or_init_processed_files(&path).unwrap();
        assert_eq!(ledger, ProcessedFilesLedger::new());
        assert!(path.exists());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_creates_a_file_with_0600_permissions() {
        let path = temp_path("perms");
        let _ = fs::remove_file(&path);

        save_processed_files(&path, &ProcessedFilesLedger::new()).unwrap();

        let perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_path("roundtrip");
        let _ = fs::remove_file(&path);

        let mut ledger = ProcessedFilesLedger::new();
        ledger.mark_processed(
            "chase_checking",
            "statement.pdf",
            "abc123",
            100.0,
            "2026-06-30",
            "6789",
            1000,
        );
        save_processed_files(&path, &ledger).unwrap();

        let loaded = load_or_init_processed_files(&path).unwrap();
        assert_eq!(loaded, ledger);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn a_malformed_file_produces_a_clear_parse_error() {
        let path = temp_path("malformed");
        fs::write(&path, "not valid json{{{").unwrap();

        let err = load_or_init_processed_files(&path).unwrap_err();
        assert!(matches!(err, ProcessedFilesStorageError::Parse(_)));

        fs::remove_file(&path).ok();
    }
}
