//! Persistence for the Plaid Item usage counter (§7.1). `item_usage.rs`
//! owns the counter's data model and threshold logic; this is the
//! storage-layer wiring its own doc comment flagged as deferred ("where
//! it's persisted... is a storage-layer concern for a later task, not
//! this one") — needed now that the Sources screen (task 24) has to
//! actually display "Plaid Items: X/10 used" across runs, not just
//! within one in-memory session. Same atomic-write + `0600` pattern as
//! `sources.rs`.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use thiserror::Error;

use crate::item_usage::ItemUsageCounter;

#[derive(Debug, Error)]
pub enum ItemUsageStorageError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("item usage counter file could not be parsed: {0}")]
    Parse(serde_json::Error),
}

/// Loads the counter, creating a fresh (zeroed) one on disk if it
/// doesn't exist yet — mirrors `sources::load_or_init`'s first-run
/// behavior.
pub fn load_or_init_item_usage(path: &Path) -> Result<ItemUsageCounter, ItemUsageStorageError> {
    if !path.exists() {
        let counter = ItemUsageCounter::new();
        save_item_usage(path, &counter)?;
        return Ok(counter);
    }

    let contents = fs::read_to_string(path)?;
    serde_json::from_str(&contents).map_err(ItemUsageStorageError::Parse)
}

/// Atomic write (temp file + rename) with `0600` permissions (§4) — not
/// a secret, but still worth protecting from accidental corruption
/// (§7.1's own reasoning for why this lives alongside `sources.yaml`).
pub fn save_item_usage(
    path: &Path,
    counter: &ItemUsageCounter,
) -> Result<(), ItemUsageStorageError> {
    let json =
        serde_json::to_string_pretty(counter).expect("ItemUsageCounter serialization cannot fail");

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
            "obol-item-usage-storage-test-{name}-{}.json",
            std::process::id()
        ))
    }

    #[test]
    fn load_or_init_creates_a_zeroed_counter_on_first_run() {
        let path = temp_path("first-run");
        let _ = fs::remove_file(&path);

        let counter = load_or_init_item_usage(&path).unwrap();
        assert_eq!(counter.count(), 0);
        assert!(path.exists());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_creates_a_file_with_0600_permissions() {
        let path = temp_path("perms");
        let _ = fs::remove_file(&path);

        save_item_usage(&path, &ItemUsageCounter::new()).unwrap();

        let perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_path("roundtrip");
        let _ = fs::remove_file(&path);

        let mut counter = ItemUsageCounter::new();
        counter.increment();
        counter.increment();
        counter.increment();
        save_item_usage(&path, &counter).unwrap();

        let loaded = load_or_init_item_usage(&path).unwrap();
        assert_eq!(loaded.count(), 3);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn a_malformed_file_produces_a_clear_parse_error() {
        let path = temp_path("malformed");
        fs::write(&path, "not valid json{{{").unwrap();

        let err = load_or_init_item_usage(&path).unwrap_err();
        assert!(matches!(err, ItemUsageStorageError::Parse(_)));

        fs::remove_file(&path).ok();
    }
}
