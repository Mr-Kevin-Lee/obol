//! Source-agnostic, provider-swappable connector architecture (spec §10).
//! `Provider` is the mechanics of *how* to reach a class of source
//! (Plaid, WebDriver, manual entry); `SourceConfig` is a config-driven
//! declaration of *one real-world account group*. Neither knows about
//! the other's specifics — the snapshot engine, retry logic, storage,
//! and dashboard only ever see this trait, never a Plaid-specific type.

use std::collections::HashMap;

use async_trait::async_trait;
use secrecy::Secret;
use thiserror::Error;

use crate::{Account, Category};

/// A config-driven declaration of one real-world account group (e.g.
/// "Chase checking + credit card"), as it will eventually be loaded from
/// `sources.yaml` (task 11 owns the actual YAML CRUD; this is the shared
/// shape both that task and this trait need).
#[derive(Debug, Clone)]
pub struct SourceConfig {
    pub id: String,
    /// The key this source is registered under in the provider registry
    /// (e.g. `"plaid"`, `"webdriver"`, `"manual_entry"`).
    pub provider: String,
    pub category: Category,
    pub account_type: String,
    pub institution: String,
    /// Salted-hash input for this source's account_key (D15) — generated
    /// once at add-time, not used by `Provider::fetch` itself, but part
    /// of the source's persisted shape.
    pub account_salt: String,
    /// Provider-declared config, shape unknown to everything except the
    /// provider that reads it (spec §10.1 — each `Provider` declares its
    /// own config schema).
    pub provider_config: serde_json::Value,
}

/// Provider-defined, opaque credential value (spec §8, §10). Each
/// provider interprets the wrapped string however it needs — a Plaid
/// access token, a JSON-encoded username/password pair for WebDriver, or
/// a plain balance string for manual entry. Wrapped in `Secret` so it
/// can't leak via an accidental `{:?}` (§4).
pub struct Credentials(pub Secret<String>);

#[derive(Debug, Clone, Error)]
pub enum ProviderError {
    /// Fails fast, never retried (§9) — bad credentials, an expired
    /// token, anything where retrying with the same input can't help.
    #[error("authentication failed: {0}")]
    Auth(String),
    /// Retried per §9's policy (3 attempts, exponential backoff, jitter)
    /// — timeouts, 5xx, connection resets, rate limiting.
    #[error("transient error: {0}")]
    Transient(String),
    /// Anything else — a malformed response, an unsupported account
    /// type, a provider-specific failure that's neither auth nor
    /// obviously transient. Not retried by default.
    #[error("provider error: {0}")]
    Other(String),
}

impl ProviderError {
    /// Whether this error should be retried (§9, D10 — feeds
    /// `tokio-retry`'s `RetryIf` condition closure once task 7 wires
    /// this in).
    pub fn is_transient(&self) -> bool {
        matches!(self, ProviderError::Transient(_))
    }
}

/// The mechanics of *how* to reach a class of source. A provider knows
/// nothing about any specific institution — it just implements this
/// trait. `async` because real providers make network calls;
/// `async_trait` is what makes this dyn-compatible for the registry's
/// `Box<dyn Provider>` (native async-fn-in-traits isn't).
#[async_trait]
pub trait Provider: Send + Sync {
    /// Returns balances for this source. Returns `Err` on failure —
    /// retry and error-capture happen in the snapshot engine (task 13),
    /// not here.
    async fn fetch(
        &self,
        source: &SourceConfig,
        credentials: Option<&Credentials>,
    ) -> Result<Vec<Box<dyn Account>>, ProviderError>;
}

pub type ProviderFactory = Box<dyn Fn() -> Box<dyn Provider> + Send + Sync>;

/// Maps a source's `provider:` string to a factory for that provider.
/// Empty for now — each provider registers itself as it's built
/// (`manual_entry` in task 15, `plaid` in task 18, `webdriver` later).
/// This task defines the registry mechanism itself; the contract is
/// tested against fakes registered ad hoc in tests, not real providers.
pub fn provider_registry() -> HashMap<&'static str, ProviderFactory> {
    HashMap::new()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{AccountStatus, Asset};

    struct FakeProvider {
        should_fail: Option<ProviderError>,
    }

    #[async_trait]
    impl Provider for FakeProvider {
        async fn fetch(
            &self,
            _source: &SourceConfig,
            _credentials: Option<&Credentials>,
        ) -> Result<Vec<Box<dyn Account>>, ProviderError> {
            if let Some(err) = &self.should_fail {
                return Err(err.clone());
            }
            Ok(vec![Box::new(Asset {
                account_key: "sha256:fake".into(),
                institution: "Fake Bank".into(),
                r#type: "checking".into(),
                balance: Some(100.0),
                status: AccountStatus::Ok,
            })])
        }
    }

    fn fake_source() -> SourceConfig {
        SourceConfig {
            id: "fake_source".into(),
            provider: "fake".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            institution: "Fake Bank".into(),
            account_salt: "salt".into(),
            provider_config: serde_json::json!({}),
        }
    }

    #[tokio::test]
    async fn successful_fetch_returns_accounts() {
        let provider = FakeProvider { should_fail: None };
        let accounts = provider.fetch(&fake_source(), None).await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].institution(), "Fake Bank");
    }

    #[tokio::test]
    async fn auth_failure_is_not_transient() {
        let provider = FakeProvider {
            should_fail: Some(ProviderError::Auth("bad credentials".into())),
        };
        let err = provider.fetch(&fake_source(), None).await.unwrap_err();
        assert!(!err.is_transient());
    }

    #[tokio::test]
    async fn transient_failure_is_transient() {
        let provider = FakeProvider {
            should_fail: Some(ProviderError::Transient("timeout".into())),
        };
        let err = provider.fetch(&fake_source(), None).await.unwrap_err();
        assert!(err.is_transient());
    }

    #[tokio::test]
    async fn other_failure_is_not_transient() {
        let provider = FakeProvider {
            should_fail: Some(ProviderError::Other("unsupported account type".into())),
        };
        let err = provider.fetch(&fake_source(), None).await.unwrap_err();
        assert!(!err.is_transient());
    }

    #[tokio::test]
    async fn registry_factory_produces_a_working_provider() {
        let mut registry = provider_registry();
        registry.insert(
            "fake",
            Box::new(|| Box::new(FakeProvider { should_fail: None }) as Box<dyn Provider>),
        );

        let factory = registry
            .get("fake")
            .expect("fake provider should be registered");
        let provider = factory();

        let accounts = provider.fetch(&fake_source(), None).await.unwrap();
        assert_eq!(accounts.len(), 1);
    }

    #[tokio::test]
    async fn credentials_are_passed_through_to_the_provider() {
        // Contract check: Provider::fetch accepts Option<&Credentials>
        // and the trait doesn't force a provider to inspect it (manual
        // entry/no-credential providers ignore it entirely) — this just
        // confirms the call compiles and works with Some(..) too.
        let provider = FakeProvider { should_fail: None };
        let creds = Credentials(Secret::new("fake-token".to_string()));
        let accounts = provider.fetch(&fake_source(), Some(&creds)).await.unwrap();
        assert_eq!(accounts.len(), 1);
    }
}
