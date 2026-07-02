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

use std::collections::HashMap;
use std::sync::Arc;

use rand::Rng;

use crate::account::{Account, AccountStatus};
use crate::pii::hash_account_number;
use crate::provider::{Credentials, Provider, ProviderError, ProviderFactory, SourceConfig};
use crate::retry::{with_retry, RetryConfig, RetryableError};
use crate::snapshot::{AccountRecord, Snapshot, Status};

const CURRENCY: &str = "USD";

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
        let provider = providers.get(&source.provider).cloned();
        let credentials = resolve_credentials(&source, credential_source);
        join_set.spawn(fetch_one(source, provider, credentials));
    }

    let mut accounts = Vec::new();
    while let Some(result) = join_set.join_next().await {
        // A panic inside fetch_one would be a genuine bug in this
        // module (fetch_one itself never panics on a provider's Err —
        // it maps it to an error record), not a per-source condition to
        // isolate, so it's allowed to propagate rather than being
        // swallowed into a fabricated record.
        accounts.extend(result.expect("a snapshot fetch task panicked"));
    }

    Snapshot {
        schema_version: crate::CURRENT_SCHEMA_VERSION,
        snapshot_id: generate_snapshot_id(),
        created_at: now_rfc3339(),
        accounts,
    }
}

/// Plaid sources resolve their access token from Keychain directly and
/// never prompt (§8) — a missing/unreadable entry becomes `None`
/// credentials, which `PlaidProvider::fetch` already turns into an
/// `Auth` error, producing a normal per-source error record. (Task 14
/// refines this into a more specific "reconnect" message per §9.1; this
/// baseline already isolates the failure correctly.) Every other source
/// goes through the interactive `CredentialSource` callback.
fn resolve_credentials(
    source: &SourceConfig,
    credential_source: &dyn CredentialSource,
) -> Option<Credentials> {
    if source.provider == "plaid" {
        crate::read_plaid_access_token(&source.id)
            .ok()
            .map(Credentials)
    } else {
        credential_source.provide(source)
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
}
