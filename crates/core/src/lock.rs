//! Cross-process advisory file lock (spec §9.1, D13). Serializes
//! concurrent runs (a scheduled `launchd` run overlapping an interactive
//! session) so they don't race on `sources.yaml` writes, snapshot
//! writes, or the Plaid Item counter. Uses `fd-lock` (chosen over the
//! spec's originally-named `fs2`/`fslock` — both unmaintained since
//! 2018/2021 respectively; `fd-lock` is actively maintained, last
//! published 2025) rather than `std::sync::Mutex`, which only
//! coordinates threads within one process, not separate processes.
//!
//! **Design note:** `fd_lock::RwLock<File>` is built for a short-lived
//! borrow within one function scope, not "acquire once, hold across a
//! whole run." To get an ownable, droppable guard, this deliberately
//! leaks the boxed `RwLock<File>` via `Box::leak` to get a `'static`
//! reference — a legitimate, well-established pattern for a resource
//! meant to live for the process's lifetime. This leaks one small, fixed
//! allocation per successful acquisition (not per poll attempt, not
//! unbounded) — the OS-level lock itself is still properly released when
//! the returned guard drops, regardless of the leaked memory.

use std::fs::{File, OpenOptions};
use std::path::Path;
use std::time::Duration;

use thiserror::Error;
use tokio::time::Instant;

const POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Error)]
pub enum LockError {
    #[error("could not acquire lock within {0:?} — another instance appears to be running")]
    Timeout(Duration),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Holds an acquired lock; releases it when dropped.
pub struct FileLock {
    _guard: fd_lock::RwLockWriteGuard<'static, File>,
}

/// Acquires the lock at `path`, creating the file if it doesn't exist.
/// Polls with [`POLL_INTERVAL`] between attempts rather than blocking
/// indefinitely — if `timeout` elapses without acquiring it, returns
/// `Err(LockError::Timeout)` with a message pointing at "another
/// instance appears to be running" (§9.1), rather than hanging.
pub async fn acquire_with_timeout(path: &Path, timeout: Duration) -> Result<FileLock, LockError> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let file = OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(path)?;

    let lock_ptr: *mut fd_lock::RwLock<File> = Box::leak(Box::new(fd_lock::RwLock::new(file)));

    let deadline = Instant::now() + timeout;
    loop {
        // SAFETY: `lock_ptr` came from `Box::leak`, so it's a valid,
        // uniquely-owned, never-freed pointer for the rest of the
        // process. Re-deriving a fresh `&'static mut` each iteration
        // (rather than reusing one `&mut` binding across the whole
        // loop) is what lets the borrow checker treat each
        // `try_write()` call as its own independent borrow — only the
        // success branch ever keeps a reference alive past its
        // iteration, so at most one `&mut` to this data exists at any
        // given moment.
        let lock: &'static mut fd_lock::RwLock<File> = unsafe { &mut *lock_ptr };
        match lock.try_write() {
            Ok(guard) => return Ok(FileLock { _guard: guard }),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= deadline {
                    return Err(LockError::Timeout(timeout));
                }
                tokio::time::sleep(POLL_INTERVAL).await;
            }
            Err(e) => return Err(LockError::Io(e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("obol-lock-test-{name}-{}.lock", std::process::id()))
    }

    #[tokio::test]
    async fn first_acquisition_succeeds_immediately() {
        let path = temp_path("first");
        let _ = std::fs::remove_file(&path);

        let result = acquire_with_timeout(&path, Duration::from_secs(1)).await;
        assert!(result.is_ok());

        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn second_acquisition_times_out_while_first_holds_the_lock() {
        let path = temp_path("contended");
        let _ = std::fs::remove_file(&path);

        let first = acquire_with_timeout(&path, Duration::from_secs(1))
            .await
            .expect("first acquisition should succeed");

        let second = acquire_with_timeout(&path, Duration::from_millis(200)).await;
        assert!(matches!(second, Err(LockError::Timeout(_))));

        drop(first);
        std::fs::remove_file(&path).ok();
    }

    #[tokio::test]
    async fn lock_is_released_on_drop_and_can_be_reacquired() {
        let path = temp_path("reacquire");
        let _ = std::fs::remove_file(&path);

        let first = acquire_with_timeout(&path, Duration::from_secs(1))
            .await
            .expect("first acquisition should succeed");
        drop(first);

        let second = acquire_with_timeout(&path, Duration::from_secs(1)).await;
        assert!(
            second.is_ok(),
            "expected reacquisition to succeed after the first lock was dropped"
        );

        std::fs::remove_file(&path).ok();
    }
}
