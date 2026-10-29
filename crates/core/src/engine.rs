//! The snapshot engine (spec §6.2 step 2, "core::snapshot::run()"). Ties
//! providers, retry, and per-source failure isolation together into one
//! `Snapshot` — the one place `Box<dyn Account>` results get turned into
//! the flat, storable `AccountRecord` shape.
//!
//! **Account trait objects never carry raw PII** (D11/§11.1 — `Asset`/
//! `Liability` only have `account_key`/`institution`/`balance`/`status`
//! fields; a provider computes the hashed `account_key` itself before
//! ever constructing one, e.g. `SimpleFinProvider` calling
//! `pii::hash_account_number` directly). So the "PII-scrubbed assembly"
//! step here is a plain field mapping, not a second scrub pass —
//! `category`/`type`/`source_id` come from the `SourceConfig` that was
//! already used to call `fetch()`, not from the returned `Account`
//! itself, consistent with D23 (one `Source` = one real-world account,
//! so the config already knows these).
//!
//! **Currency is hardcoded to `"USD"`** for v1 — neither the `Account`
//! trait nor any `SourceConfig` carries a currency field, and every
//! institution this project targets (§7) is USD-denominated. Revisit if
//! that ever stops being true.
//!
//! **Error records' `account_key` is a documented approximation.** A
//! successful fetch hashes the *real* external account identifier
//! (§11.1) so the same real account keeps the same key across a source
//! rename. But when `fetch()` fails outright, that real identifier was
//! never learned — there's nothing to hash. Error records fall back to
//! hashing `source.id` instead, which is stable across error runs but
//! **not** guaranteed to match the key a successful run for the same
//! source would produce. This only affects trend-analysis continuity
//! (FR10, a stretch goal not in v1 scope) for a source that flips
//! between erroring and succeeding across runs — not a correctness
//! issue for any v1 requirement.
//!
//! **§9.1 failure-mode wiring** also lives here: [`run_and_save`]
//! guarantees a save failure never discards the snapshot that was just
//! fetched (best-effort, not blocking, persistence).

use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

use rand::Rng;

use crate::account::{Account, AccountStatus};
use crate::pii::hash_account_number;
use crate::provider::{Credentials, Provider, ProviderError, ProviderFactory, SourceConfig};
use crate::retry::{with_retry, RetryConfig, RetryableError};
use crate::snapshot::{AccountRecord, Snapshot, Status};
use crate::storage::StorageError;

const CURRENCY: &str = "USD";

/// Interactive credential prompting, threaded through core without
/// giving core a UI-crate dependency (spec §6.2, decision D12). Called
/// once per source that needs it — never for SimpleFIN sources, whose
/// Access URL the engine resolves itself (env var, then Keychain).
pub trait CredentialSource: Send + Sync {
    fn provide(&self, source: &SourceConfig) -> Option<Credentials>;
}

/// Runs one full snapshot pass over `sources` (spec §6.2 step 2):
/// providers are instantiated once per provider *type*, not per source
/// (deduped via `registry`); each source is fetched concurrently, with
/// §9's retry policy applied per fetch; a source that fails — a fetch
/// error, a retry-exhausting timeout, or an unrecognized `provider:`
/// name — produces a `status: "error"` record rather than aborting the
/// run (§9's per-source isolation). Returns the assembled `Snapshot`;
/// saving it to disk is the caller's job (§6.2 step 3, `storage.rs`).
pub async fn run(
    sources: &[SourceConfig],
    registry: &HashMap<&'static str, ProviderFactory>,
    credential_source: &dyn CredentialSource,
) -> Snapshot {
    run_with(
        sources,
        registry,
        credential_source,
        std::env::var(crate::simplefin_provider::ACCESS_URL_ENV_VAR).ok(),
        || crate::read_simplefin_access_url().ok(),
    )
    .await
}

/// `run` with the environment value and Keychain reader passed in, so
/// tests never touch the real ones. The SimpleFIN Access URL is one value
/// shared by every SimpleFIN source, so it is resolved once per run (and
/// only if some source needs it), not once per source.
async fn run_with(
    sources: &[SourceConfig],
    registry: &HashMap<&'static str, ProviderFactory>,
    credential_source: &dyn CredentialSource,
    env_value: Option<String>,
    read_keychain: impl FnOnce() -> Option<secrecy::Secret<String>>,
) -> Snapshot {
    let simplefin_url = sources
        .iter()
        .any(|s| s.provider == "simplefin")
        .then(|| crate::simplefin_provider::resolve_access_url(env_value, read_keychain))
        .flatten();

    let mut providers: HashMap<String, Arc<dyn Provider>> = HashMap::new();
    for source in sources {
        if providers.contains_key(&source.provider) {
            continue;
        }
        if let Some(factory) = registry.get(source.provider.as_str()) {
            providers.insert(source.provider.clone(), Arc::from(factory()));
        }
    }

    let mut join_set = tokio::task::JoinSet::new();
    for source in sources.iter().cloned() {
        // SimpleFIN sources share the Access URL resolved above and never
        // prompt; every other source goes through the interactive
        // `CredentialSource` callback.
        let credentials = if source.provider == "simplefin" {
            simplefin_url.clone().map(Credentials)
        } else {
            credential_source.provide(&source)
        };
        let provider = providers.get(&source.provider).cloned();
        join_set.spawn(fetch_one(source, provider, credentials));
    }

    let mut accounts = Vec::new();
    while let Some(result) = join_set.join_next().await {
        // A panic inside fetch_one would be a genuine bug in this
        // module (fetch_one itself never panics on a provider's Err —
        // it maps it to an error record), not a per-source condition to
        // isolate, so it's allowed to propagate rather than being
        // swallowed into a fabricated record.
        //
        // Every record gets audit-logged right here — the one place
        // every path through fetch_one and the Keychain short-circuit
        // above both funnel through, so this can't be missed by adding
        // a new failure path later. §4's audit requirement is
        // deliberately narrow: timestamp + source_id + ok/error, never
        // a balance or account_key (source_id is a locally-chosen
        // label, not tied to the real account at all).
        let records = result.expect("a snapshot fetch task panicked");
        for record in &records {
            log_outcome(record);
        }
        accounts.extend(records);
    }

    Snapshot {
        schema_version: crate::CURRENT_SCHEMA_VERSION,
        snapshot_id: generate_snapshot_id(),
        created_at: now_rfc3339(),
        accounts,
    }
}

/// Logs one source's outcome for the audit trail (spec §4: "timestamp,
/// which sources succeeded/failed — no balances, no identifiers... so
/// connection health is visible over time without the log itself
/// becoming a sensitive artifact"). `tracing` events already carry a
/// timestamp; `source_id` is the local, user-chosen label from
/// `sources.yaml` (e.g. `"chase_checking"`), never the real account
/// number or the salted `account_key`. Deliberately never passes
/// `record.balance()` or `record.account_key()` to a `tracing` macro —
/// that omission is the actual security property this function exists
/// to enforce, verified by this module's own audit-log test.
fn log_outcome(record: &AccountRecord) {
    match record.status() {
        Status::Ok => {
            tracing::info!(source_id = %record.source_id(), "source fetch succeeded");
        }
        Status::Error | Status::Unknown => {
            tracing::warn!(
                source_id = %record.source_id(),
                error = %record.error_message().unwrap_or("unknown error"),
                "source fetch failed"
            );
        }
    }
}

/// The outcome of [`run_and_save`] — `snapshot` is always the freshly
/// fetched data, regardless of whether persisting it succeeded.
pub struct RunAndSaveResult {
    pub snapshot: Snapshot,
    /// `Some` if `storage::save_snapshot` failed (§9.1: "best-effort,
    /// not blocking" persistence — disk full, permissions, unexpected
    /// I/O). The caller (CLI/TUI) still renders `snapshot`, but should
    /// surface this as a clear "this run's data was not written to
    /// history" warning rather than silently dropping it.
    pub save_error: Option<StorageError>,
}

/// Runs a snapshot pass and attempts to save it (spec §6.2 steps 2–3),
/// but never lets a save failure discard the snapshot that was just
/// fetched — the whole point of §9.1's best-effort persistence
/// guarantee. Whatever `run()` produced is always returned; a save
/// failure only ever shows up in `save_error`.
pub async fn run_and_save(
    sources: &[SourceConfig],
    registry: &HashMap<&'static str, ProviderFactory>,
    credential_source: &dyn CredentialSource,
    storage_dir: &Path,
) -> RunAndSaveResult {
    let snapshot = run(sources, registry, credential_source).await;
    let save_error = crate::save_snapshot(storage_dir, &snapshot).err();
    RunAndSaveResult {
        snapshot,
        save_error,
    }
}

async fn fetch_one(
    source: SourceConfig,
    provider: Option<Arc<dyn Provider>>,
    credentials: Option<Credentials>,
) -> Vec<AccountRecord> {
    let Some(provider) = provider else {
        return vec![error_record(
            &source,
            format!("unknown provider: '{}'", source.provider),
        )];
    };

    // Arc-wrapped so each retry attempt gets its own cheap handle
    // without requiring `Credentials`/`SourceConfig` themselves to be
    // `Clone` (the same reasoning as retry.rs's own `Arc<Mutex<F>>>`
    // wrapping — attempts run strictly sequentially, so this is purely
    // about satisfying the closure's ownership requirements, not real
    // contention).
    let source = Arc::new(source);
    let credentials = Arc::new(credentials);

    let result = with_retry(
        RetryConfig::spec_default(),
        {
            let provider = provider.clone();
            let source = source.clone();
            let credentials = credentials.clone();
            move || {
                let provider = provider.clone();
                let source = source.clone();
                let credentials = credentials.clone();
                async move { provider.fetch(&source, credentials.as_ref().as_ref()).await }
            }
        },
        ProviderError::is_transient,
    )
    .await;

    match result {
        Ok(accounts) => accounts
            .iter()
            .map(|account| account_to_record(&source, account.as_ref()))
            .collect(),
        Err(RetryableError::Timeout) => {
            vec![error_record(&source, "operation timed out".to_string())]
        }
        Err(RetryableError::Operation(provider_err)) => {
            vec![error_record(&source, provider_err.to_string())]
        }
    }
}

fn account_to_record(source: &SourceConfig, account: &dyn Account) -> AccountRecord {
    let (status, error_message) = match account.status() {
        AccountStatus::Ok => (Status::Ok, None),
        AccountStatus::Error { message } => (Status::Error, Some(message.clone())),
    };
    AccountRecord {
        account_key: account.account_key().to_string(),
        source_id: source.id.clone(),
        institution: account.institution().to_string(),
        category: source.category,
        account_type: source.account_type.clone(),
        balance: account.balance(),
        currency: CURRENCY.to_string(),
        status,
        error_message,
        holdings: account.holdings().map(|h| h.to_vec()),
    }
}

fn error_record(source: &SourceConfig, message: String) -> AccountRecord {
    AccountRecord {
        account_key: hash_account_number(&source.id, &source.account_salt),
        source_id: source.id.clone(),
        institution: source.institution.clone(),
        category: source.category,
        account_type: source.account_type.clone(),
        balance: None,
        currency: CURRENCY.to_string(),
        status: Status::Error,
        error_message: Some(message),
        holdings: None,
    }
}

/// A random, locally-generated identifier — not a spec-mandated UUID
/// format, just something unique enough to name a snapshot file by
/// (`storage.rs` uses this as the filename). Reuses `rand`, already a
/// dependency (sources.rs's salt generation), rather than adding a
/// dedicated `uuid` crate for one random string.
fn generate_snapshot_id() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn now_rfc3339() -> String {
    time::OffsetDateTime::now_utc()
        .format(&time::format_description::well_known::Rfc3339)
        .expect("RFC3339 formatting of the current time should never fail")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::account::{Asset, Liability};
    use crate::snapshot::Category;
    use async_trait::async_trait;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::{Duration, Instant};

    struct FakeProvider {
        calls: Arc<AtomicUsize>,
        delay: Duration,
        result: FakeResult,
    }

    #[derive(Clone)]
    enum FakeResult {
        Ok,
        Err(ProviderError),
    }

    #[async_trait]
    impl Provider for FakeProvider {
        async fn fetch(
            &self,
            source: &SourceConfig,
            _credentials: Option<&Credentials>,
        ) -> Result<Vec<Box<dyn Account>>, ProviderError> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            if !self.delay.is_zero() {
                tokio::time::sleep(self.delay).await;
            }
            match &self.result {
                FakeResult::Ok => {
                    let account: Box<dyn Account> = match source.category {
                        Category::Liability => Box::new(Liability {
                            account_key: "sha256:fake-real-account-number".into(),
                            institution: "Fake Bank".into(),
                            r#type: source.account_type.clone(),
                            balance: Some(500.0),
                            status: AccountStatus::Ok,
                        }),
                        _ => Box::new(Asset {
                            account_key: "sha256:fake-real-account-number".into(),
                            institution: "Fake Bank".into(),
                            r#type: source.account_type.clone(),
                            balance: Some(100.0),
                            status: AccountStatus::Ok,
                            holdings: None,
                        }),
                    };
                    Ok(vec![account])
                }
                FakeResult::Err(e) => Err(e.clone()),
            }
        }
    }

    struct FakeCredentialSource {
        calls: Arc<AtomicUsize>,
    }

    impl CredentialSource for FakeCredentialSource {
        fn provide(&self, _source: &SourceConfig) -> Option<Credentials> {
            self.calls.fetch_add(1, Ordering::SeqCst);
            None
        }
    }

    fn fake_source(id: &str, provider: &str, category: Category) -> SourceConfig {
        SourceConfig {
            id: id.into(),
            provider: provider.into(),
            category,
            account_type: "checking".into(),
            institution: "Fake Bank".into(),
            account_salt: "test-salt".into(),
            provider_config: serde_json::json!({}),
        }
    }

    /// Builds a registry entry whose factory bumps `instantiations` each
    /// time it's called, so a test can assert how many times a provider
    /// *type* was actually instantiated (not just how many sources
    /// reference it) — the mechanism `run()` uses for dedup-by-type.
    fn counting_registry_entry(
        instantiations: Arc<AtomicUsize>,
        calls: Arc<AtomicUsize>,
        delay: Duration,
        result: FakeResult,
    ) -> ProviderFactory {
        Box::new(move || {
            instantiations.fetch_add(1, Ordering::SeqCst);
            Box::new(FakeProvider {
                calls: calls.clone(),
                delay,
                result: result.clone(),
            }) as Box<dyn Provider>
        })
    }

    #[tokio::test]
    async fn dedupes_provider_instantiation_by_type() {
        let instantiations = Arc::new(AtomicUsize::new(0));
        let calls = Arc::new(AtomicUsize::new(0));

        let mut registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        registry.insert(
            "fake",
            counting_registry_entry(
                instantiations.clone(),
                calls,
                Duration::ZERO,
                FakeResult::Ok,
            ),
        );

        let sources = vec![
            fake_source("source_a", "fake", Category::Asset),
            fake_source("source_b", "fake", Category::Asset),
        ];
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let snapshot = run(&sources, &registry, &credential_source).await;

        assert_eq!(
            instantiations.load(Ordering::SeqCst),
            1,
            "two sources sharing one provider type should only instantiate it once"
        );
        assert_eq!(snapshot.accounts.len(), 2);
    }

    #[tokio::test]
    async fn fetches_multiple_sources_concurrently_not_sequentially() {
        let delay = Duration::from_millis(150);
        let calls = Arc::new(AtomicUsize::new(0));

        // Different provider *names* so each gets its own instance —
        // dedup is covered by a separate test; this one is purely about
        // whether independent fetches overlap in time.
        let mut registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        for name in ["slow_a", "slow_b", "slow_c"] {
            registry.insert(
                name,
                counting_registry_entry(
                    Arc::new(AtomicUsize::new(0)),
                    calls.clone(),
                    delay,
                    FakeResult::Ok,
                ),
            );
        }

        let sources = vec![
            fake_source("source_a", "slow_a", Category::Asset),
            fake_source("source_b", "slow_b", Category::Asset),
            fake_source("source_c", "slow_c", Category::Asset),
        ];
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let start = Instant::now();
        let snapshot = run(&sources, &registry, &credential_source).await;
        let elapsed = start.elapsed();

        assert_eq!(snapshot.accounts.len(), 3);
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        assert!(
            elapsed < delay * 2,
            "3 concurrent ~150ms fetches took {elapsed:?} — expected well under {:?} if truly concurrent",
            delay * 2
        );
    }

    #[tokio::test]
    async fn assembles_a_successful_fetch_using_the_source_configs_category_and_type() {
        let mut registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        registry.insert(
            "fake",
            counting_registry_entry(
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
                Duration::ZERO,
                FakeResult::Ok,
            ),
        );
        let sources = vec![fake_source("chase_checking", "fake", Category::Asset)];
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let snapshot = run(&sources, &registry, &credential_source).await;

        assert_eq!(snapshot.accounts.len(), 1);
        let record = &snapshot.accounts[0];
        assert_eq!(record.source_id(), "chase_checking");
        assert_eq!(record.category(), Category::Asset);
        assert_eq!(record.account_type(), "checking");
        assert_eq!(record.balance(), Some(100.0));
        assert_eq!(record.status(), Status::Ok);
        assert_eq!(record.currency(), "USD");
    }

    #[tokio::test]
    async fn one_failing_source_does_not_affect_a_succeeding_one() {
        let mut registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        registry.insert(
            "good",
            counting_registry_entry(
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
                Duration::ZERO,
                FakeResult::Ok,
            ),
        );
        registry.insert(
            "bad",
            counting_registry_entry(
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
                Duration::ZERO,
                FakeResult::Err(ProviderError::Auth("bad credentials".into())),
            ),
        );

        let sources = vec![
            fake_source("good_source", "good", Category::Asset),
            fake_source("bad_source", "bad", Category::Asset),
        ];
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let snapshot = run(&sources, &registry, &credential_source).await;

        assert_eq!(snapshot.accounts.len(), 2);
        let good = snapshot
            .accounts
            .iter()
            .find(|r| r.source_id() == "good_source")
            .unwrap();
        let bad = snapshot
            .accounts
            .iter()
            .find(|r| r.source_id() == "bad_source")
            .unwrap();
        assert_eq!(good.status(), Status::Ok);
        assert_eq!(bad.status(), Status::Error);
        assert!(bad.error_message().unwrap().contains("bad credentials"));
    }

    #[tokio::test]
    async fn an_unknown_provider_produces_an_isolated_error_record() {
        let registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        let sources = vec![fake_source(
            "mystery_source",
            "does_not_exist",
            Category::Asset,
        )];
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let snapshot = run(&sources, &registry, &credential_source).await;

        assert_eq!(snapshot.accounts.len(), 1);
        let record = &snapshot.accounts[0];
        assert_eq!(record.status(), Status::Error);
        assert!(record
            .error_message()
            .unwrap()
            .contains("unknown provider: 'does_not_exist'"));
    }

    /// Records the credentials each fetch is given, then fails (the
    /// outcome is irrelevant to the credential tests).
    struct RecordingProvider {
        seen: Arc<std::sync::Mutex<Vec<Option<String>>>>,
    }

    #[async_trait]
    impl Provider for RecordingProvider {
        async fn fetch(
            &self,
            _source: &SourceConfig,
            credentials: Option<&Credentials>,
        ) -> Result<Vec<Box<dyn Account>>, ProviderError> {
            self.seen
                .lock()
                .unwrap()
                .push(credentials.map(|c| secrecy::ExposeSecret::expose_secret(&c.0).clone()));
            Err(ProviderError::Other("recorded".into()))
        }
    }

    fn recording_registry(
        seen: &Arc<std::sync::Mutex<Vec<Option<String>>>>,
    ) -> HashMap<&'static str, ProviderFactory> {
        let seen = seen.clone();
        let mut registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        registry.insert(
            "simplefin",
            Box::new(move || {
                Box::new(RecordingProvider { seen: seen.clone() }) as Box<dyn Provider>
            }),
        );
        registry
    }

    fn three_simplefin_sources() -> Vec<SourceConfig> {
        ["a", "b", "c"]
            .map(|id| fake_source(id, "simplefin", Category::Asset))
            .to_vec()
    }

    #[tokio::test]
    async fn every_simplefin_source_gets_the_url_from_a_single_keychain_read() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let reads = Arc::new(AtomicUsize::new(0));
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let reads_in_closure = reads.clone();
        run_with(
            &three_simplefin_sources(),
            &recording_registry(&seen),
            &credential_source,
            None,
            move || {
                reads_in_closure.fetch_add(1, Ordering::SeqCst);
                Some(secrecy::Secret::new("https://from-keychain".to_string()))
            },
        )
        .await;

        assert_eq!(
            (seen.lock().unwrap().clone(), reads.load(Ordering::SeqCst)),
            (vec![Some("https://from-keychain".to_string()); 3], 1)
        );
    }

    #[tokio::test]
    async fn a_simplefin_env_value_wins_and_the_keychain_is_not_read() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        run_with(
            &three_simplefin_sources(),
            &recording_registry(&seen),
            &credential_source,
            Some("https://from-env".to_string()),
            || panic!("the Keychain must not be read when the env value is set"),
        )
        .await;

        assert_eq!(
            seen.lock().unwrap().clone(),
            vec![Some("https://from-env".to_string()); 3]
        );
    }

    #[tokio::test]
    async fn the_keychain_is_not_read_when_no_source_uses_simplefin() {
        let registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        run_with(
            &[fake_source("only", "generic", Category::Asset)],
            &registry,
            &credential_source,
            None,
            || panic!("the Keychain must not be read without a SimpleFIN source"),
        )
        .await;
    }

    #[tokio::test]
    async fn only_non_simplefin_sources_use_the_credential_source() {
        let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
        let calls = Arc::new(AtomicUsize::new(0));
        let credential_source = FakeCredentialSource {
            calls: calls.clone(),
        };
        let sources = vec![
            fake_source("sf", "simplefin", Category::Asset),
            fake_source("other", "generic", Category::Asset),
        ];

        run_with(
            &sources,
            &recording_registry(&seen),
            &credential_source,
            None,
            || None,
        )
        .await;

        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn snapshot_gets_a_fresh_schema_version_and_snapshot_id() {
        let registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let snapshot = run(&[], &registry, &credential_source).await;

        assert_eq!(snapshot.schema_version, crate::CURRENT_SCHEMA_VERSION);
        assert!(!snapshot.snapshot_id.is_empty());
        assert!(snapshot.accounts.is_empty());
    }

    #[tokio::test]
    async fn two_runs_produce_different_snapshot_ids() {
        let registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let first = run(&[], &registry, &credential_source).await;
        let second = run(&[], &registry, &credential_source).await;

        assert_ne!(first.snapshot_id, second.snapshot_id);
    }

    #[tokio::test]
    async fn run_and_save_returns_the_snapshot_even_when_saving_fails() {
        // A path whose parent component is an existing *file*, not a
        // directory, so create_dir_all inside save_snapshot fails —
        // simulates the disk-full/permissions class of failure §9.1
        // describes without needing real disk exhaustion.
        let blocking_file = std::env::temp_dir().join(format!(
            "obol-engine-test-run-and-save-blocker-{}",
            std::process::id()
        ));
        std::fs::write(&blocking_file, "not a directory").unwrap();
        let unwritable_dir = blocking_file.join("snapshots");

        let mut registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        registry.insert(
            "fake",
            counting_registry_entry(
                Arc::new(AtomicUsize::new(0)),
                Arc::new(AtomicUsize::new(0)),
                Duration::ZERO,
                FakeResult::Ok,
            ),
        );
        let sources = vec![fake_source("chase_checking", "fake", Category::Asset)];
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let result = run_and_save(&sources, &registry, &credential_source, &unwritable_dir).await;

        assert_eq!(
            result.snapshot.accounts.len(),
            1,
            "the fetched snapshot must still be returned even though saving failed"
        );
        assert!(result.save_error.is_some());

        std::fs::remove_file(&blocking_file).ok();
    }

    /// Spec §4: audit log output must never contain a balance or
    /// account_key, only timestamp + source_id + ok/error. Tests
    /// `log_outcome` directly and synchronously (a plain `#[test]`, not
    /// `#[tokio::test]`) — it runs in `run()`'s own task, never inside
    /// a `JoinSet`-spawned one, so there's no async/threading machinery
    /// actually relevant here to route around.
    #[test]
    fn audit_log_never_contains_a_balance_or_account_key() {
        let captured = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));

        #[derive(Clone)]
        struct VecWriter(Arc<std::sync::Mutex<Vec<u8>>>);
        impl std::io::Write for VecWriter {
            fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
                self.0.lock().unwrap().extend_from_slice(buf);
                Ok(buf.len())
            }
            fn flush(&mut self) -> std::io::Result<()> {
                Ok(())
            }
        }

        let writer = VecWriter(captured.clone());
        let subscriber = tracing_subscriber::fmt()
            .with_writer(move || writer.clone())
            .with_ansi(false)
            .finish();

        let ok_record = AccountRecord {
            account_key: "sha256:should-never-appear-in-logs".into(),
            source_id: "ok_source".into(),
            institution: "Fake Bank".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            balance: Some(918_273.45),
            currency: "USD".into(),
            status: Status::Ok,
            error_message: None,
            holdings: None,
        };
        let bad_record = AccountRecord {
            account_key: "sha256:should-also-never-appear".into(),
            source_id: "bad_source".into(),
            institution: "Fake Bank".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            balance: None,
            currency: "USD".into(),
            status: Status::Error,
            error_message: Some("timeout".into()),
            holdings: None,
        };

        tracing::subscriber::with_default(subscriber, || {
            log_outcome(&ok_record);
            log_outcome(&bad_record);
        });

        let output = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
        assert!(
            !output.contains("918273.45") && !output.contains("918_273.45"),
            "audit log must never contain a balance, got:\n{output}"
        );
        assert!(
            !output.contains("should-never-appear-in-logs")
                && !output.contains("should-also-never-appear"),
            "audit log must never contain an account_key, got:\n{output}"
        );
        assert!(
            output.contains("ok_source") && output.contains("bad_source"),
            "audit log should still contain the (non-sensitive) source_id, got:\n{output}"
        );
    }
}
