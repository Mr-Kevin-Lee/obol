//! Snapshot storage (spec §11.2, §4). Each snapshot is saved as its own
//! versioned JSON file, named by `snapshot_id`, and never overwritten —
//! so a crash mid-write only ever risks that write's own temp file, never
//! a previously-saved snapshot. `core::snapshot::run()` (task 13) decides
//! what to do if `save_snapshot` returns `Err` (§9.1's "best-effort, not
//! blocking" persistence); this module only guarantees the write itself
//! is atomic.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

use thiserror::Error;

use crate::migration::{load_snapshot_json, LoadedSnapshot, MigrationError};
use crate::snapshot::Snapshot;

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("failed to serialize snapshot: {0}")]
    Serialize(serde_json::Error),
    #[error("failed to parse snapshot: {0}")]
    Parse(#[from] MigrationError),
}

/// Saves a snapshot as `<dir>/<snapshot_id>.json` (atomic write — temp
/// file + rename — with `0600` file / `0700` directory permissions, §4).
/// Returns the path written to.
pub fn save_snapshot(dir: &Path, snapshot: &Snapshot) -> Result<PathBuf, StorageError> {
    fs::create_dir_all(dir)?;
    fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;

    let path = dir.join(format!("{}.json", snapshot.snapshot_id));
    let temp_path = dir.join(format!("{}.json.tmp", snapshot.snapshot_id));

    let json = serde_json::to_string_pretty(snapshot).map_err(StorageError::Serialize)?;
    {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(json.as_bytes())?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temp_path, &path)?;

    Ok(path)
}

/// Loads a single snapshot file, running the same migration chain as any
/// other snapshot load (§11.3).
pub fn load_snapshot(path: &Path) -> Result<LoadedSnapshot, StorageError> {
    let raw = fs::read_to_string(path)?;
    Ok(load_snapshot_json(&raw)?)
}

/// Loads the `n` most recently saved snapshots from `dir`, newest first
/// (§6.2 step 4 — feeds "last updated N runs ago" and the stretch trend
/// chart, independent of the fresh snapshot just computed in the same
/// run). Sorted by file modification time rather than requiring a
/// sortable filename scheme. A missing directory (no snapshots saved
/// yet) returns an empty list rather than an error. A snapshot file that
/// fails to load (corrupted, truncated by an interrupted write that
/// somehow still got renamed) is skipped rather than failing the whole
/// call — one bad historical snapshot shouldn't block viewing the
/// others, the same per-item isolation principle as §9.1.
pub fn load_recent_snapshots(dir: &Path, n: usize) -> Result<Vec<Snapshot>, StorageError> {
    if !dir.exists() {
        return Ok(Vec::new());
    }

    let mut entries: Vec<(std::time::SystemTime, PathBuf)> = fs::read_dir(dir)?
        .filter_map(|entry| entry.ok())
        .filter(|entry| entry.path().extension().is_some_and(|ext| ext == "json"))
        .filter_map(|entry| {
            let modified = entry.metadata().ok()?.modified().ok()?;
            Some((modified, entry.path()))
        })
        .collect();

    entries.sort_by(|a, b| b.0.cmp(&a.0));

    let snapshots = entries
        .into_iter()
        .take(n)
        .filter_map(|(_, path)| load_snapshot(&path).ok().map(|loaded| loaded.snapshot))
        .collect();

    Ok(snapshots)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountRecord, Category, Status};
    use std::time::Duration;

    fn temp_dir(name: &str) -> PathBuf {
        std::env::temp_dir().join(format!("obol-storage-test-{name}-{}", std::process::id()))
    }

    fn cleanup(dir: &Path) {
        let _ = fs::remove_dir_all(dir);
    }

    fn fake_snapshot(id: &str) -> Snapshot {
        Snapshot {
            schema_version: 1,
            snapshot_id: id.to_string(),
            created_at: "2026-06-30T09:15:00-07:00".into(),
            accounts: vec![fake_record()],
        }
    }

    fn fake_record() -> AccountRecord {
        AccountRecord {
            account_key: "sha256:9f2a...".into(),
            source_id: "chase_checking".into(),
            institution: "Chase".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            balance: Some(4213.55),
            currency: "USD".into(),
            status: Status::Ok,
            error_message: None,
        }
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir("roundtrip");
        cleanup(&dir);

        let snapshot = fake_snapshot("snap-1");
        save_snapshot(&dir, &snapshot).unwrap();

        let path = dir.join("snap-1.json");
        let loaded = load_snapshot(&path).unwrap();
        assert_eq!(loaded.snapshot, snapshot);
        assert!(loaded.forward_compat_warning.is_none());

        cleanup(&dir);
    }

    #[test]
    fn save_creates_directory_and_file_with_correct_permissions() {
        let dir = temp_dir("perms");
        cleanup(&dir);

        let snapshot = fake_snapshot("snap-perms");
        let path = save_snapshot(&dir, &snapshot).unwrap();

        let dir_perms = fs::metadata(&dir).unwrap().permissions();
        assert_eq!(dir_perms.mode() & 0o777, 0o700);

        let file_perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(file_perms.mode() & 0o777, 0o600);

        cleanup(&dir);
    }

    #[test]
    fn saving_two_snapshots_does_not_overwrite_the_first() {
        let dir = temp_dir("no-overwrite");
        cleanup(&dir);

        save_snapshot(&dir, &fake_snapshot("snap-a")).unwrap();
        save_snapshot(&dir, &fake_snapshot("snap-b")).unwrap();

        let loaded_a = load_snapshot(&dir.join("snap-a.json")).unwrap();
        let loaded_b = load_snapshot(&dir.join("snap-b.json")).unwrap();
        assert_eq!(loaded_a.snapshot.snapshot_id, "snap-a");
        assert_eq!(loaded_b.snapshot.snapshot_id, "snap-b");

        cleanup(&dir);
    }

    #[test]
    fn a_stray_temp_file_from_an_interrupted_write_does_not_corrupt_the_prior_snapshot() {
        let dir = temp_dir("interrupted");
        cleanup(&dir);

        // A successful prior save.
        save_snapshot(&dir, &fake_snapshot("snap-good")).unwrap();

        // Simulate a crash partway through writing a second snapshot:
        // the temp file exists (partial/garbage content), but the
        // rename to the real path never happened.
        fs::write(dir.join("snap-crashed.json.tmp"), "not valid json{{{").unwrap();

        // The prior snapshot is completely unaffected.
        let loaded = load_snapshot(&dir.join("snap-good.json")).unwrap();
        assert_eq!(loaded.snapshot.snapshot_id, "snap-good");

        // The stray .tmp file is invisible to a real snapshot's path —
        // there is no snap-crashed.json.
        assert!(!dir.join("snap-crashed.json").exists());

        cleanup(&dir);
    }

    #[test]
    fn load_recent_snapshots_returns_newest_first_and_ignores_stray_tmp_files() {
        let dir = temp_dir("recent-order");
        cleanup(&dir);

        save_snapshot(&dir, &fake_snapshot("snap-1")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        save_snapshot(&dir, &fake_snapshot("snap-2")).unwrap();
        std::thread::sleep(Duration::from_millis(20));
        save_snapshot(&dir, &fake_snapshot("snap-3")).unwrap();
        fs::write(dir.join("stray.json.tmp"), "garbage").unwrap();

        let recent = load_recent_snapshots(&dir, 10).unwrap();
        let ids: Vec<&str> = recent.iter().map(|s| s.snapshot_id.as_str()).collect();
        assert_eq!(ids, vec!["snap-3", "snap-2", "snap-1"]);

        cleanup(&dir);
    }

    #[test]
    fn load_recent_snapshots_limits_to_n() {
        let dir = temp_dir("recent-limit");
        cleanup(&dir);

        for i in 0..5 {
            save_snapshot(&dir, &fake_snapshot(&format!("snap-{i}"))).unwrap();
            std::thread::sleep(Duration::from_millis(10));
        }

        let recent = load_recent_snapshots(&dir, 2).unwrap();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].snapshot_id, "snap-4");
        assert_eq!(recent[1].snapshot_id, "snap-3");

        cleanup(&dir);
    }

    #[test]
    fn load_recent_snapshots_on_missing_directory_returns_empty() {
        let dir = temp_dir("does-not-exist");
        cleanup(&dir);

        let recent = load_recent_snapshots(&dir, 5).unwrap();
        assert!(recent.is_empty());
    }

    #[test]
    fn load_recent_snapshots_skips_a_corrupted_file_but_returns_the_others() {
        let dir = temp_dir("recent-corrupted");
        cleanup(&dir);

        save_snapshot(&dir, &fake_snapshot("snap-ok")).unwrap();
        fs::write(dir.join("snap-corrupted.json"), "{ not valid json").unwrap();

        let recent = load_recent_snapshots(&dir, 10).unwrap();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].snapshot_id, "snap-ok");

        cleanup(&dir);
    }

    #[test]
    fn load_snapshot_on_a_missing_file_is_an_io_error() {
        let dir = temp_dir("missing-file");
        cleanup(&dir);

        let err = load_snapshot(&dir.join("does-not-exist.json")).unwrap_err();
        assert!(matches!(err, StorageError::Io(_)));
    }
}
