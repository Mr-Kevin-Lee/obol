//! Retry/backoff wrapper (spec §9, decision D10). Wraps an async
//! operation with exponential backoff + jitter via `tokio-retry`'s
//! `RetryIf`, and a per-attempt hard timeout via `tokio::time::timeout`
//! (which `tokio-retry` doesn't provide itself). The caller supplies an
//! `is_retryable` predicate so auth failures can fail fast instead of
//! retrying (§9) — a timeout is always treated as retryable, since by
//! definition it's the kind of transient condition retries exist for.

use std::future::Future;
use std::time::Duration;

use tokio_retry::strategy::{jitter, ExponentialBackoff};
use tokio_retry::RetryIf;

/// Retry parameters, configurable so tests don't need to wait on the
/// real spec-mandated durations. Production code should use
/// [`RetryConfig::spec_default`].
#[derive(Debug, Clone, Copy)]
pub struct RetryConfig {
    /// Total attempts, including the first — §9 says 3.
    pub max_attempts: usize,
    /// Delay before the second attempt; doubles each attempt after that
    /// (before jitter is applied) — §9 says 2s.
    pub initial_backoff: Duration,
    /// Hard timeout per attempt — §9 says 15s.
    pub per_attempt_timeout: Duration,
}

impl RetryConfig {
    /// The policy §9 mandates: 3 attempts, 2s initial exponential
    /// backoff (doubling, ±20% jitter via `tokio-retry`'s own `jitter`),
    /// 15s hard timeout per attempt.
    pub fn spec_default() -> Self {
        Self {
            max_attempts: 3,
            initial_backoff: Duration::from_secs(2),
            per_attempt_timeout: Duration::from_secs(15),
        }
    }
}

#[derive(Debug)]
pub enum RetryableError<E> {
    /// The per-attempt hard timeout elapsed before the operation
    /// finished. Always retried — a timeout is exactly the kind of
    /// transient condition this policy exists for.
    Timeout,
    /// The operation itself returned an error. Whether this gets
    /// retried is up to the caller's `is_retryable` predicate.
    Operation(E),
}

/// Runs `operation` with §9's retry policy. `is_retryable` decides
/// whether a given operation error should be retried at all — auth
/// failures should return `false` here to fail fast.
pub async fn with_retry<F, Fut, T, E>(
    config: RetryConfig,
    operation: F,
    is_retryable: impl Fn(&E) -> bool,
) -> Result<T, RetryableError<E>>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, E>>,
{
    let retries = config.max_attempts.saturating_sub(1);
    let strategy = ExponentialBackoff::from_millis(config.initial_backoff.as_millis() as u64)
        .map(jitter)
        .take(retries);

    let per_attempt_timeout = config.per_attempt_timeout;
    // Attempts run strictly sequentially (RetryIf awaits each one fully
    // before trying the next), so this Mutex never actually contends —
    // it's here purely so each closure invocation can independently
    // reach `operation` without the returned async block borrowing past
    // the closure call that produced it.
    let operation = std::sync::Arc::new(tokio::sync::Mutex::new(operation));

    RetryIf::start(
        strategy,
        move || {
            let operation = operation.clone();
            async move {
                let mut op = operation.lock().await;
                match tokio::time::timeout(per_attempt_timeout, op()).await {
                    Ok(Ok(value)) => Ok(value),
                    Ok(Err(e)) => Err(RetryableError::Operation(e)),
                    Err(_elapsed) => Err(RetryableError::Timeout),
                }
            }
        },
        |err: &RetryableError<E>| match err {
            RetryableError::Timeout => true,
            RetryableError::Operation(e) => is_retryable(e),
        },
    )
    .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Instant;
    use tokio::sync::Mutex;

    fn fast_config() -> RetryConfig {
        RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(10),
            per_attempt_timeout: Duration::from_millis(200),
        }
    }

    #[test]
    fn spec_default_matches_the_spec_exactly() {
        let config = RetryConfig::spec_default();
        assert_eq!(config.max_attempts, 3);
        assert_eq!(config.initial_backoff, Duration::from_secs(2));
        assert_eq!(config.per_attempt_timeout, Duration::from_secs(15));
    }

    #[tokio::test]
    async fn succeeds_on_first_attempt_without_retrying() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();

        let result: Result<u32, RetryableError<&str>> = with_retry(
            fast_config(),
            move || {
                let calls = calls_clone.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Ok(42)
                }
            },
            |_: &&str| true,
        )
        .await;

        assert!(matches!(result, Ok(42)));
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_a_retryable_error_and_eventually_succeeds() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();

        let result: Result<u32, RetryableError<&str>> = with_retry(
            fast_config(),
            move || {
                let calls = calls_clone.clone();
                async move {
                    let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
                    if n < 2 {
                        Err("transient")
                    } else {
                        Ok(99)
                    }
                }
            },
            |_: &&str| true,
        )
        .await;

        assert!(matches!(result, Ok(99)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn exhausts_all_attempts_then_gives_up() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();

        let result: Result<u32, RetryableError<&str>> = with_retry(
            fast_config(),
            move || {
                let calls = calls_clone.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err("always fails")
                }
            },
            |_: &&str| true,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
    }

    #[tokio::test]
    async fn non_retryable_error_fails_fast() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();

        let result: Result<u32, RetryableError<&str>> = with_retry(
            fast_config(),
            move || {
                let calls = calls_clone.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    Err("auth failure")
                }
            },
            |_: &&str| false,
        )
        .await;

        assert!(result.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "a non-retryable error should not be retried"
        );
    }

    #[tokio::test]
    async fn per_attempt_timeout_is_enforced_and_treated_as_retryable() {
        let calls = Arc::new(AtomicUsize::new(0));
        let calls_clone = calls.clone();
        let config = RetryConfig {
            max_attempts: 2,
            initial_backoff: Duration::from_millis(5),
            per_attempt_timeout: Duration::from_millis(20),
        };

        let result: Result<u32, RetryableError<&str>> = with_retry(
            config,
            move || {
                let calls = calls_clone.clone();
                async move {
                    calls.fetch_add(1, Ordering::SeqCst);
                    // Always longer than the 20ms per-attempt timeout.
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok::<u32, &str>(1)
                }
            },
            |_: &&str| true,
        )
        .await;

        assert!(matches!(result, Err(RetryableError::Timeout)));
        assert_eq!(calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn backoff_delays_roughly_increase_between_attempts() {
        let timestamps: Arc<Mutex<Vec<Instant>>> = Arc::new(Mutex::new(Vec::new()));
        let timestamps_clone = timestamps.clone();
        let config = RetryConfig {
            max_attempts: 3,
            initial_backoff: Duration::from_millis(30),
            per_attempt_timeout: Duration::from_millis(500),
        };

        let _: Result<u32, RetryableError<&str>> = with_retry(
            config,
            move || {
                let timestamps = timestamps_clone.clone();
                async move {
                    timestamps.lock().await.push(Instant::now());
                    Err("always fails")
                }
            },
            |_: &&str| true,
        )
        .await;

        let recorded = timestamps.lock().await;
        assert_eq!(recorded.len(), 3);
        let gap1 = recorded[1].duration_since(recorded[0]);
        let gap2 = recorded[2].duration_since(recorded[1]);
        // With jitter (±20%) this won't be exact, but the second gap
        // (backoff ~60ms before jitter) should clearly be larger than
        // the first (~30ms before jitter) — loose bounds, not asserting
        // tokio-retry's own jitter math precisely.
        assert!(
            gap2 > gap1,
            "expected increasing backoff between attempts, got {gap1:?} then {gap2:?}"
        );
    }
}
