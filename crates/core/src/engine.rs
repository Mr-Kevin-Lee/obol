//! The snapshot engine (spec §6.2 step 2, "core::snapshot::run()"). Ties
//! providers, retry, and per-source failure isolation together into one
//! `Snapshot` — the one place `Box<dyn Account>` results get turned into
//! the flat, storable `AccountRecord` shape.
//!
//! **Account trait objects never carry raw PII** (D11/§11.1 — `Asset`/
//! `Liability` only have `account_key`/`institution`/`balance`/`status`
//! fields; a provider computes the hashed `account_key` itself before
//! ever constructing one, e.g. `PlaidProvider` calling
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
//! **§9.1 failure-mode wiring** also lives here: a Plaid source whose
//! Keychain read fails gets a specific "reconnect this source" message
//! rather than being routed through `PlaidProvider` (which can't tell a
//! Keychain failure apart from any other reason it got no credentials),
//! and [`run_and_save`] guarantees a save failure never discards the
//! snapshot that was just fetched (best-effort, not blocking,
//! persistence).

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

/// §9.1: "a Plaid Keychain read failure is treated as a relink signal,
/// not a generic error" — this exact wording is what points the user at
/// the Sources screen's existing Reconnect flow, rather than a generic
/// auth-failure message that doesn't say what to do about it.
const PLAID_RECONNECT_MESSAGE: &str =
    "Plaid access token could not be read from Keychain — reconnect this source from the Sources screen";

/// Interactive credential prompting, threaded through core without
/// giving core a UI-crate dependency (spec §6.2, decision D12). Called
/// once per source that needs it — never for Plaid sources, which
/// resolve their access token from Keychain internally (§8).
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
        match resolve_credentials(&source, credential_source) {
            CredentialResolution::PlaidKeychainUnavailable => {
                // §9.1: treated as a relink signal, not routed through
                // fetch_one/PlaidProvider at all — that would only ever
                // produce a generic "no access token" auth error, not
                // this specific, actionable message.
                let record = error_record(&source, PLAID_RECONNECT_MESSAGE.to_string());
                join_set.spawn(async move { vec![record] });
            }
            CredentialResolution::Resolved(credentials) => {
                let provider = providers.get(&source.provider).cloned();
                join_set.spawn(fetch_one(source, provider, credentials));
            }
        }
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

enum CredentialResolution {
    Resolved(Option<Credentials>),
    /// Specifically a Plaid source whose Keychain read failed (§9.1) —
    /// kept distinct from `Resolved(None)` so `run()` can short-circuit
    /// straight to the reconnect-signal error record instead of routing
    /// through `PlaidProvider`, which has no way to tell "Keychain read
    /// failed" apart from any other reason it got no credentials.
    PlaidKeychainUnavailable,
}

/// Dev/testing-only escape hatch for the parked Keychain signing bug
/// (D24) — mirrors the precedent already established for this app's own
/// Plaid `client_id`/`secret` (D20): an env var is fine for verifying
/// real behavior against a real Plaid Item, never how the shipped app
/// holds this credential at rest. Applies to every Plaid source in this
/// run alike (one Item's token can legitimately back several sources,
/// D23), not per-source — this is a blunt bridge, not a real
/// per-source credential store.
const DEV_ACCESS_TOKEN_ENV_VAR: &str = "PLAID_DEV_ACCESS_TOKEN";

/// Plaid sources resolve their access token from Keychain directly and
/// never prompt (§8). Every other source goes through the interactive
/// `CredentialSource` callback.
fn resolve_credentials(
    source: &SourceConfig,
    credential_source: &dyn CredentialSource,
) -> CredentialResolution {
    if source.provider == "plaid" {
        if let Ok(token) = std::env::var(DEV_ACCESS_TOKEN_ENV_VAR) {
            eprintln!(
                "warning: source '{}' is using {DEV_ACCESS_TOKEN_ENV_VAR} \
                 (dev/testing bridge, not real credential storage — see D24)",
                source.id
            );
            return CredentialResolution::Resolved(Some(Credentials(secrecy::Secret::new(token))));
        }
        match crate::read_plaid_access_token(&source.id) {
            Ok(token) => CredentialResolution::Resolved(Some(Credentials(token))),
            Err(_) => CredentialResolution::PlaidKeychainUnavailable,
        }
    } else {
        CredentialResolution::Resolved(credential_source.provide(source))
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

    #[tokio::test]
    async fn plaid_sources_never_call_the_credential_source_callback() {
        // No real Keychain entry exists for this made-up source id, so
        // read_plaid_access_token fails fast with a real (harmless,
        // read-only) "not found" — no signing/entitlement needed for a
        // read, unlike store (see keychain.rs, D24).
        let registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        let sources = vec![
            fake_source(
                "plaid_source_with_no_keychain_entry",
                "plaid",
                Category::Asset,
            ),
            fake_source("generic_source", "generic", Category::Asset),
        ];
        let calls = Arc::new(AtomicUsize::new(0));
        let credential_source = FakeCredentialSource {
            calls: calls.clone(),
        };

        run(&sources, &registry, &credential_source).await;

        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "CredentialSource should be called once, for the non-Plaid source only"
        );
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
    async fn a_plaid_keychain_read_failure_produces_a_reconnect_message_not_a_generic_one() {
        // Same "no real entry for this made-up id" mechanism as the
        // callback test above — a real, fast, read-only Keychain miss,
        // not something requiring a signed binary (D24).
        let registry: HashMap<&'static str, ProviderFactory> = HashMap::new();
        let sources = vec![fake_source(
            "plaid_source_never_linked",
            "plaid",
            Category::Asset,
        )];
        let credential_source = FakeCredentialSource {
            calls: Arc::new(AtomicUsize::new(0)),
        };

        let snapshot = run(&sources, &registry, &credential_source).await;

        assert_eq!(snapshot.accounts.len(), 1);
        let record = &snapshot.accounts[0];
        assert_eq!(record.status(), Status::Error);
        let message = record.error_message().unwrap();
        assert!(
            message.contains("reconnect"),
            "expected a reconnect-signal message, got: {message}"
        );
        assert!(
            !message.contains("no access token"),
            "should not fall through to PlaidProvider's generic auth message: {message}"
        );
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
