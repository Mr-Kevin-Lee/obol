//! Local record of SimpleFIN requests, so a bug (or just many dashboard
//! visits) can't push past the Bridge's quota. The Bridge asks for <=24
//! requests a day, "replenished throughout the day", and disables Access
//! Tokens that go well beyond that, so this is a rolling 24-hour log of
//! request times, not a per-calendar-day counter.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};

const WINDOW_SECS: i64 = 24 * 60 * 60;

/// Unix timestamps (seconds) of requests made.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct UsageLog {
    requests: Vec<i64>,
}

/// The cap was already reached inside the window.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct BudgetExhausted {
    pub used: usize,
    pub cap: usize,
}

impl UsageLog {
    /// Requests still inside the trailing 24 hours (test helper; the
    /// runtime path only needs `try_record`).
    #[cfg(test)]
    pub(crate) fn count_in_window(&self, now: i64) -> usize {
        self.requests
            .iter()
            .filter(|&&at| now - at < WINDOW_SECS)
            .count()
    }

    /// Drops expired entries, then records a request at `now` unless `cap`
    /// requests are already inside the window. Returns the new count.
    pub(crate) fn try_record(&mut self, now: i64, cap: usize) -> Result<usize, BudgetExhausted> {
        self.requests.retain(|&at| now - at < WINDOW_SECS);
        if self.requests.len() >= cap {
            return Err(BudgetExhausted {
                used: self.requests.len(),
                cap,
            });
        }
        self.requests.push(now);
        Ok(self.requests.len())
    }
}

/// A missing or unreadable file reads as an empty log: a damaged usage
/// file must not stop balances from loading.
pub(crate) fn load_usage_log(path: &Path) -> UsageLog {
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// Atomic (temp file + rename), `0600` like the rest of the storage dir.
pub(crate) fn save_usage_log(path: &Path, log: &UsageLog) -> std::io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let temp_path = path.with_extension("json.tmp");
    {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(serde_json::to_string(log)?.as_bytes())?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temp_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;

    const T0: i64 = 1_800_000_000;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "obol-usage-test-{name}-{}/simplefin_usage.json",
            std::process::id()
        ))
    }

    fn log_with(requests: &[i64]) -> UsageLog {
        UsageLog {
            requests: requests.to_vec(),
        }
    }

    #[test]
    fn an_empty_log_has_a_zero_count() {
        assert_eq!(UsageLog::default().count_in_window(T0), 0);
    }

    #[test]
    fn count_includes_requests_inside_the_window() {
        let log = log_with(&[T0 - 100, T0 - 50]);
        assert_eq!(log.count_in_window(T0), 2);
    }

    #[test]
    fn count_excludes_requests_older_than_24_hours() {
        let log = log_with(&[T0 - WINDOW_SECS - 1, T0 - 10]);
        assert_eq!(log.count_in_window(T0), 1);
    }

    #[test]
    fn a_request_exactly_24_hours_old_has_expired() {
        let log = log_with(&[T0 - WINDOW_SECS]);
        assert_eq!(log.count_in_window(T0), 0);
    }

    #[test]
    fn a_request_just_under_24_hours_old_still_counts() {
        let log = log_with(&[T0 - WINDOW_SECS + 1]);
        assert_eq!(log.count_in_window(T0), 1);
    }

    #[test]
    fn try_record_succeeds_and_returns_the_new_count() {
        let mut log = UsageLog::default();
        assert_eq!(log.try_record(T0, 3), Ok(1));
    }

    #[test]
    fn try_record_allows_requests_up_to_the_cap() {
        let mut log = UsageLog::default();
        log.try_record(T0, 3).unwrap();
        log.try_record(T0 + 1, 3).unwrap();
        assert_eq!(log.try_record(T0 + 2, 3), Ok(3));
    }

    #[test]
    fn try_record_rejects_the_request_that_would_exceed_the_cap() {
        let mut log = log_with(&[T0, T0 + 1, T0 + 2]);
        assert_eq!(
            log.try_record(T0 + 3, 3),
            Err(BudgetExhausted { used: 3, cap: 3 })
        );
    }

    #[test]
    fn a_rejected_request_is_not_recorded() {
        let mut log = log_with(&[T0, T0 + 1, T0 + 2]);
        let _ = log.try_record(T0 + 3, 3);
        assert_eq!(log.count_in_window(T0 + 3), 3);
    }

    #[test]
    fn expired_requests_free_up_budget() {
        let mut log = log_with(&[T0, T0 + 1, T0 + 2]);
        assert_eq!(log.try_record(T0 + WINDOW_SECS + 1, 3), Ok(2));
    }

    #[test]
    fn try_record_drops_expired_entries_from_the_log() {
        let mut log = log_with(&[T0 - WINDOW_SECS - 5]);
        log.try_record(T0, 3).unwrap();
        assert_eq!(log.requests, vec![T0]);
    }

    #[test]
    fn a_missing_file_loads_as_an_empty_log() {
        assert_eq!(load_usage_log(&temp_path("missing")), UsageLog::default());
    }

    #[test]
    fn a_corrupt_file_loads_as_an_empty_log() {
        let path = temp_path("corrupt");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, "not json {").unwrap();
        assert_eq!(load_usage_log(&path), UsageLog::default());
    }

    #[test]
    fn a_saved_log_loads_back_identically() {
        let path = temp_path("roundtrip");
        let log = log_with(&[T0, T0 + 5]);
        save_usage_log(&path, &log).unwrap();
        assert_eq!(load_usage_log(&path), log);
    }

    #[test]
    fn saving_creates_the_parent_directory() {
        let path = temp_path("mkdir");
        let _ = fs::remove_dir_all(path.parent().unwrap());
        save_usage_log(&path, &UsageLog::default()).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn the_saved_file_is_private_to_the_user() {
        let path = temp_path("perms");
        save_usage_log(&path, &UsageLog::default()).unwrap();
        let mode = fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
    }

    #[test]
    fn saving_replaces_an_existing_file() {
        let path = temp_path("replace");
        save_usage_log(&path, &log_with(&[T0])).unwrap();
        save_usage_log(&path, &log_with(&[T0 + 9])).unwrap();
        assert_eq!(load_usage_log(&path), log_with(&[T0 + 9]));
    }
}
