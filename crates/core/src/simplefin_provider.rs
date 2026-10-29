//! Ties the hand-rolled SimpleFIN client (`simplefin.rs`) into the
//! `Provider` trait (§10). Like Plaid (D23), a `Source` is one account, so
//! `fetch()` finds the account matching `provider_config.simplefin_account_id`.
//!
//! One Access URL covers every account, and the Bridge asks for <=24
//! requests/day, so a short-lived cache lets all of a run's sources share a
//! single `GET /accounts`.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use async_trait::async_trait;
use secrecy::{ExposeSecret, Secret};
use tokio::sync::Mutex;

use crate::pii::hash_account_number;
use crate::simplefin::{AccountSet, SimpleFinAccount, SimpleFinClient, SimpleFinError};
use crate::simplefin_usage::{load_usage_log, save_usage_log};
use crate::{Account, AccountStatus, Asset, Category, Credentials, Liability};
use crate::{Provider, ProviderError, ProviderFactory, SourceConfig};

/// Long enough to cover one run's sources, short enough that a second run
/// a minute later sees fresh balances.
const OK_TTL: Duration = Duration::from_secs(60);

/// A failure is remembered only briefly: long enough that a run's sources
/// share one failed request instead of each sending their own (and the
/// engine's retries, which back off ~2s then ~4s, mostly reuse it), short
/// enough that a later retry can still succeed.
const ERROR_TTL: Duration = Duration::from_secs(5);

/// Below the 24 requests/day the Bridge asks for, as a margin.
const REQUEST_CAP: usize = 20;

/// The last outcome (success *or* failure), so a failing run doesn't
/// re-ask once per source.
struct CachedSet {
    access_url: Secret<String>,
    fetched_at: Instant,
    result: Result<Arc<AccountSet>, ProviderError>,
}

pub struct SimpleFinProvider {
    client: SimpleFinClient,
    cache: Mutex<Option<CachedSet>>,
    ok_ttl: Duration,
    err_ttl: Duration,
    /// Where the rolling request log lives; `None` means unlimited.
    usage_path: Option<PathBuf>,
    request_cap: usize,
}

impl SimpleFinProvider {
    pub fn new(client: SimpleFinClient) -> Self {
        Self {
            client,
            cache: Mutex::new(None),
            ok_ttl: OK_TTL,
            err_ttl: ERROR_TTL,
            usage_path: None,
            request_cap: REQUEST_CAP,
        }
    }

    /// Like `new`, but every request is checked against and recorded in the
    /// rolling 24-hour log at `usage_path`.
    pub fn with_usage_log(client: SimpleFinClient, usage_path: PathBuf) -> Self {
        Self {
            usage_path: Some(usage_path),
            ..Self::new(client)
        }
    }

    /// Holds the lock across the request, so sources fetched concurrently
    /// still produce a single `GET /accounts`.
    async fn account_set(
        &self,
        access_url: &Secret<String>,
    ) -> Result<Arc<AccountSet>, ProviderError> {
        let mut cache = self.cache.lock().await;
        if let Some(cached) = cache.as_ref() {
            let ttl = if cached.result.is_ok() {
                self.ok_ttl
            } else {
                self.err_ttl
            };
            if cached.access_url.expose_secret() == access_url.expose_secret()
                && cached.fetched_at.elapsed() < ttl
            {
                return cached.result.clone();
            }
        }

        let result = self.request(access_url).await;
        *cache = Some(CachedSet {
            access_url: Secret::new(access_url.expose_secret().clone()),
            fetched_at: Instant::now(),
            result: result.clone(),
        });
        result
    }

    /// One real `GET /accounts`, after checking and recording it against
    /// the request budget. A failed request still counts: the Bridge does.
    async fn request(&self, access_url: &Secret<String>) -> Result<Arc<AccountSet>, ProviderError> {
        self.reserve_request()?;
        self.client
            .get_balances(access_url)
            .await
            .map(Arc::new)
            .map_err(map_simplefin_error)
    }

    fn reserve_request(&self) -> Result<(), ProviderError> {
        let Some(path) = &self.usage_path else {
            return Ok(());
        };
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let mut log = load_usage_log(path);
        log.try_record(now, self.request_cap).map_err(|e| {
            ProviderError::Other(format!(
                "SimpleFIN request budget used up: {} requests in the last 24h (cap {}; \
                 the Bridge asks for 24 or fewer a day) — try again later",
                e.used, e.cap
            ))
        })?;
        // Failing to persist only weakens the guard; don't block balances.
        if let Err(err) = save_usage_log(path, &log) {
            tracing::warn!(error = %err, "could not save the SimpleFIN usage log");
        }
        Ok(())
    }
}

#[async_trait]
impl Provider for SimpleFinProvider {
    async fn fetch(
        &self,
        source: &SourceConfig,
        credentials: Option<&Credentials>,
    ) -> Result<Vec<Box<dyn Account>>, ProviderError> {
        let access_url = credentials.map(|c| &c.0).ok_or_else(|| {
            ProviderError::Auth(
                "no SimpleFIN Access URL — link SimpleFIN or set SIMPLEFIN_ACCESS_URL".into(),
            )
        })?;

        let account_id = source
            .provider_config
            .get("simplefin_account_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::Other(format!(
                    "source {} is missing provider_config.simplefin_account_id",
                    source.id
                ))
            })?;

        let set = self.account_set(access_url).await?;

        let account = set
            .accounts
            .iter()
            .find(|a| a.id == account_id)
            .ok_or_else(|| {
                // No Bridge account id here: this text reaches the audit
                // log, which carries no identifiers.
                ProviderError::Other(format!(
                    "this source's account was not found in SimpleFIN's response (source {}) \
                     — it may have been removed from the Bridge{}",
                    source.id,
                    bridge_errors_suffix(&set)
                ))
            })?;

        if let Some(err) = relevant_error(&set, account) {
            return Err(err);
        }

        let balance = account
            .balance
            .trim()
            .parse::<f64>()
            .ok()
            .filter(|b| b.is_finite())
            .ok_or_else(|| {
                ProviderError::Other(format!(
                    "SimpleFIN returned a non-numeric balance for source {}",
                    source.id
                ))
            })?;
        let account_key = hash_account_number(&account.id, &source.account_salt);

        let account: Box<dyn Account> = match source.category {
            Category::Asset => Box::new(Asset {
                account_key,
                institution: source.institution.clone(),
                r#type: source.account_type.clone(),
                balance: Some(balance),
                status: AccountStatus::Ok,
                holdings: None,
            }),
            Category::Liability => Box::new(Liability {
                account_key,
                institution: source.institution.clone(),
                r#type: source.account_type.clone(),
                balance: Some(balance.abs()),
                status: AccountStatus::Ok,
            }),
            Category::Unknown => {
                return Err(ProviderError::Other(format!(
                    "source {} has an unrecognized category — can't tell if this is an \
                     asset or a liability",
                    source.id
                )))
            }
        };

        Ok(vec![account])
    }
}

pub(crate) const ACCESS_URL_ENV_VAR: &str = "SIMPLEFIN_ACCESS_URL";

/// Where the engine finds the Access URL (a dev bridge): the env var wins,
/// session-scoped and never written to disk (and earns a warning, once per
/// run); otherwise the Keychain. A blank env var counts as unset. Takes
/// both as inputs so the order is testable without touching the real
/// environment or Keychain.
pub(crate) fn resolve_access_url(
    env_value: Option<String>,
    read_keychain: impl FnOnce() -> Option<Secret<String>>,
) -> Option<Secret<String>> {
    let from_env = env_value.filter(|v| !v.trim().is_empty());
    if from_env.is_some() {
        eprintln!(
            "warning: SimpleFIN sources are using {ACCESS_URL_ENV_VAR} (dev/testing bridge, \
             not real credential storage)"
        );
    }
    from_env
        .map(|v| Secret::new(v.trim().to_string()))
        .or_else(read_keychain)
}

/// Registers the provider under `"simplefin"`. Unconditional: it has no
/// app-level credentials to gate on (the Access URL is resolved per run by
/// the engine). Requests are counted in the log at `usage_path`.
pub fn register(registry: &mut HashMap<&'static str, ProviderFactory>, usage_path: PathBuf) {
    registry.insert(
        "simplefin",
        Box::new(move || {
            Box::new(SimpleFinProvider::with_usage_log(
                SimpleFinClient::default(),
                usage_path.clone(),
            )) as Box<dyn Provider>
        }),
    );
}

fn is_auth_code(code: &str) -> bool {
    code.starts_with("con.auth") || code.starts_with("gen.auth")
}

/// The first `errlist` entry that concerns this account. The narrowest
/// scope wins: an entry naming an account concerns only that account (even
/// if it also names the connection), and one naming only a connection
/// concerns that connection's accounts. An unscoped entry only counts when
/// it is an auth failure: the Bridge also reports non-fatal warnings
/// (e.g. quota overruns) there alongside valid balances, and those must not
/// fail every account. Other accounts' and connections' errors don't fail
/// this one (a stale bank must not block the healthy ones).
fn relevant_error(set: &AccountSet, account: &SimpleFinAccount) -> Option<ProviderError> {
    let entry = set
        .errlist
        .iter()
        .find(|e| match (&e.account_id, &e.conn_id) {
            (Some(account_id), _) => *account_id == account.id,
            (None, Some(conn_id)) => *conn_id == account.conn_id,
            (None, None) => is_auth_code(&e.code),
        })?;
    let detail = format!("[{}] {}", entry.code, sanitize(&entry.msg));
    Some(if is_auth_code(&entry.code) {
        ProviderError::Auth(detail)
    } else {
        ProviderError::Other(detail)
    })
}

/// The protocol says to sanitize server-supplied messages before showing
/// them: drop control characters (terminal escapes) and cap the length.
pub fn sanitize(msg: &str) -> String {
    msg.chars().filter(|c| !c.is_control()).take(200).collect()
}

/// The Bridge's own `errlist`, for a "not found" message: a failing
/// connection usually omits its accounts, so this is often the real reason.
fn bridge_errors_suffix(set: &AccountSet) -> String {
    if set.errlist.is_empty() {
        return String::new();
    }
    let reported: Vec<String> = set
        .errlist
        .iter()
        .map(|e| format!("[{}] {}", e.code, sanitize(&e.msg)))
        .collect();
    format!("; the Bridge reported: {}", reported.join("; "))
}

fn map_simplefin_error(err: SimpleFinError) -> ProviderError {
    let detail = err.to_string();
    match err {
        SimpleFinError::Transport(_) => ProviderError::Transient(detail),
        SimpleFinError::AccessDenied(_) => ProviderError::Auth(detail),
        SimpleFinError::UnexpectedStatus { status } if status >= 500 => {
            ProviderError::Transient(detail)
        }
        _ => ProviderError::Other(detail),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// A fresh, empty usage log path unique to this test.
    fn usage_path(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "obol-sfprov-{name}-{}/simplefin_usage.json",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        path
    }

    fn provider_with_usage(path: &std::path::Path, cap: usize) -> SimpleFinProvider {
        SimpleFinProvider {
            request_cap: cap,
            // No response caching, so every fetch reaches the budget check.
            ok_ttl: Duration::ZERO,
            err_ttl: Duration::ZERO,
            ..SimpleFinProvider::with_usage_log(SimpleFinClient::default(), path.to_path_buf())
        }
    }

    async fn mount_expecting(server: &MockServer, status: u16, expected_requests: u64) {
        let template = if status == 200 {
            ResponseTemplate::new(200).set_body_json(two_account_body())
        } else {
            ResponseTemplate::new(status)
        };
        Mock::given(method("GET"))
            .and(path("/simplefin/accounts"))
            .respond_with(template)
            .expect(expected_requests)
            .mount(server)
            .await;
    }

    async fn fetch_chk(
        provider: &SimpleFinProvider,
        server: &MockServer,
    ) -> Result<Vec<Box<dyn Account>>, ProviderError> {
        provider
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(server)),
            )
            .await
    }

    #[tokio::test]
    async fn a_failure_is_shared_by_sources_fetched_right_after() {
        let server = MockServer::start().await;
        mount_expecting(&server, 500, 1).await;
        let provider = SimpleFinProvider::new(SimpleFinClient::default());

        let first = fetch_chk(&provider, &server).await;
        let second = provider
            .fetch(
                &fake_source(Category::Liability, "acc_card"),
                Some(&credentials_for(&server)),
            )
            .await;

        assert!(first.is_err() && second.is_err());
    }

    #[tokio::test]
    async fn an_expired_success_is_fetched_again() {
        let server = MockServer::start().await;
        mount_expecting(&server, 200, 2).await;
        let provider = SimpleFinProvider {
            ok_ttl: Duration::ZERO,
            ..SimpleFinProvider::new(SimpleFinClient::default())
        };

        fetch_chk(&provider, &server).await.unwrap();
        fetch_chk(&provider, &server).await.unwrap();
    }

    #[tokio::test]
    async fn an_expired_failure_is_fetched_again() {
        let server = MockServer::start().await;
        mount_expecting(&server, 500, 2).await;
        let provider = SimpleFinProvider {
            err_ttl: Duration::ZERO,
            ..SimpleFinProvider::new(SimpleFinClient::default())
        };

        let _ = fetch_chk(&provider, &server).await;
        let _ = fetch_chk(&provider, &server).await;
    }

    #[tokio::test]
    async fn requests_past_the_cap_are_refused_without_calling_the_bridge() {
        let server = MockServer::start().await;
        mount_expecting(&server, 200, 1).await;
        let path = usage_path("cap");
        let provider = provider_with_usage(&path, 1);

        fetch_chk(&provider, &server).await.unwrap();
        let err = fetch_chk(&provider, &server).await.unwrap_err();

        assert!(err.to_string().contains("budget"), "got: {err}");
    }

    #[tokio::test]
    async fn a_budget_refusal_is_not_retried() {
        let server = MockServer::start().await;
        mount_expecting(&server, 200, 1).await;
        let path = usage_path("noretry");
        let provider = provider_with_usage(&path, 1);

        fetch_chk(&provider, &server).await.unwrap();
        let err = fetch_chk(&provider, &server).await.unwrap_err();

        assert!(!err.is_transient());
    }

    #[tokio::test]
    async fn a_failed_request_still_counts_against_the_budget() {
        let server = MockServer::start().await;
        mount_expecting(&server, 500, 1).await;
        let path = usage_path("failcounts");
        let provider = provider_with_usage(&path, 1);

        let _ = fetch_chk(&provider, &server).await;
        let err = fetch_chk(&provider, &server).await.unwrap_err();

        assert!(err.to_string().contains("budget"), "got: {err}");
    }

    #[tokio::test]
    async fn the_budget_is_shared_across_provider_instances() {
        let server = MockServer::start().await;
        mount_expecting(&server, 200, 1).await;
        let path = usage_path("shared");

        fetch_chk(&provider_with_usage(&path, 1), &server)
            .await
            .unwrap();
        let err = fetch_chk(&provider_with_usage(&path, 1), &server)
            .await
            .unwrap_err();

        assert!(err.to_string().contains("budget"), "got: {err}");
    }

    #[tokio::test]
    async fn each_request_is_recorded_in_the_usage_log() {
        let server = MockServer::start().await;
        mount_expecting(&server, 200, 2).await;
        let path = usage_path("recorded");
        let provider = provider_with_usage(&path, 5);

        fetch_chk(&provider, &server).await.unwrap();
        fetch_chk(&provider, &server).await.unwrap();

        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        assert_eq!(
            crate::simplefin_usage::load_usage_log(&path).count_in_window(now),
            2
        );
    }

    #[tokio::test]
    async fn a_provider_without_a_usage_log_is_not_limited() {
        let server = MockServer::start().await;
        mount_expecting(&server, 200, 25).await;
        let provider = SimpleFinProvider {
            ok_ttl: Duration::ZERO,
            ..SimpleFinProvider::new(SimpleFinClient::default())
        };

        for _ in 0..25 {
            fetch_chk(&provider, &server).await.unwrap();
        }
    }

    fn keychain_with(url: &str) -> impl FnOnce() -> Option<Secret<String>> + '_ {
        move || Some(Secret::new(url.to_string()))
    }

    fn unreachable_keychain() -> Option<Secret<String>> {
        panic!("the Keychain must not be read when the env var is set")
    }

    #[test]
    fn resolve_access_url_prefers_the_env_var_without_reading_the_keychain() {
        let creds = resolve_access_url(Some("https://env".into()), unreachable_keychain).unwrap();
        assert_eq!(creds.expose_secret(), "https://env");
    }

    #[test]
    fn resolve_access_url_falls_back_to_the_keychain() {
        let creds = resolve_access_url(None, keychain_with("https://keychain")).unwrap();
        assert_eq!(creds.expose_secret(), "https://keychain");
    }

    #[test]
    fn resolve_access_url_treats_a_blank_env_var_as_unset() {
        let creds =
            resolve_access_url(Some("  ".into()), keychain_with("https://keychain")).unwrap();
        assert_eq!(creds.expose_secret(), "https://keychain");
    }

    #[test]
    fn resolve_access_url_is_none_when_nothing_is_configured() {
        assert!(resolve_access_url(None, || None).is_none());
    }

    #[tokio::test]
    async fn register_adds_a_working_simplefin_provider() {
        let server = MockServer::start().await;
        mount_accounts(&server, two_account_body()).await;

        let mut registry = HashMap::new();
        register(&mut registry, usage_path("register"));

        let provider = registry.get("simplefin").expect("simplefin registered")();
        let accounts = provider
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap();
        assert_eq!(accounts.len(), 1);
    }

    fn credentials_for(server: &MockServer) -> Credentials {
        let base = server.uri().replace("http://", "http://user:pass@");
        Credentials(Secret::new(format!("{base}/simplefin")))
    }

    fn fake_source(category: Category, account_id: &str) -> SourceConfig {
        SourceConfig {
            id: format!("src_{account_id}"),
            provider: "simplefin".into(),
            category,
            account_type: "checking".into(),
            institution: "Chase".into(),
            account_salt: "test-salt".into(),
            provider_config: json!({ "simplefin_account_id": account_id }),
        }
    }

    async fn mount_accounts(server: &MockServer, body: serde_json::Value) {
        Mock::given(method("GET"))
            .and(path("/simplefin/accounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(body))
            .mount(server)
            .await;
    }

    fn two_account_body() -> serde_json::Value {
        json!({
            "errlist": [],
            "connections": [
                { "conn_id": "c1", "name": "Chase" },
                { "conn_id": "c2", "name": "Amex" }
            ],
            "accounts": [
                { "id": "acc_chk", "name": "Checking", "conn_id": "c1", "balance": "110.50" },
                { "id": "acc_card", "name": "Card", "conn_id": "c2", "balance": "-250.00" }
            ]
        })
    }

    #[tokio::test]
    async fn fetch_returns_the_matching_account_as_an_asset() {
        let server = MockServer::start().await;
        mount_accounts(&server, two_account_body()).await;

        let accounts = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].balance(), Some(110.5));
        assert_eq!(accounts[0].institution(), "Chase");
    }

    #[tokio::test]
    async fn fetch_hashes_the_account_id_with_the_source_salt() {
        let server = MockServer::start().await;
        mount_accounts(&server, two_account_body()).await;

        let accounts = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap();
        assert_eq!(
            accounts[0].account_key(),
            hash_account_number("acc_chk", "test-salt")
        );
    }

    // ASSUMPTION (unverified against a real card): SimpleFIN reports what a
    // card owes as a negative number, and `Liability.balance` is the
    // positive amount owed, so the sign is normalized with abs().
    #[tokio::test]
    async fn fetch_builds_a_liability_with_a_positive_owed_amount() {
        let server = MockServer::start().await;
        mount_accounts(&server, two_account_body()).await;

        let accounts = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Liability, "acc_card"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap();
        assert_eq!(accounts[0].net_worth_contribution(), -250.0);
    }

    #[tokio::test]
    async fn fetch_without_credentials_is_an_auth_error() {
        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(&fake_source(Category::Asset, "acc_chk"), None)
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }

    #[tokio::test]
    async fn fetch_without_simplefin_account_id_in_config_is_an_error() {
        let server = MockServer::start().await;
        let mut source = fake_source(Category::Asset, "acc_chk");
        source.provider_config = json!({});

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(&source, Some(&credentials_for(&server)))
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)));
    }

    #[tokio::test]
    async fn fetch_errors_when_the_account_is_not_in_the_response() {
        let server = MockServer::start().await;
        mount_accounts(&server, two_account_body()).await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_gone"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)));
    }

    #[tokio::test]
    async fn fetch_errors_on_an_unrecognized_category() {
        let server = MockServer::start().await;
        mount_accounts(&server, two_account_body()).await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Unknown, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)));
    }

    #[tokio::test]
    async fn fetch_errors_when_the_balance_is_not_a_number() {
        let server = MockServer::start().await;
        mount_accounts(
            &server,
            json!({
                "errlist": [], "connections": [],
                "accounts": [{ "id": "acc_chk", "name": "C", "conn_id": "c1", "balance": "n/a" }]
            }),
        )
        .await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)));
    }

    #[tokio::test]
    async fn fetch_errors_when_the_balance_is_not_finite() {
        for bad in ["NaN", "inf", "-inf", "1e999"] {
            let server = MockServer::start().await;
            mount_accounts(
                &server,
                json!({
                    "errlist": [], "connections": [],
                    "accounts": [{ "id": "acc_chk", "name": "C", "conn_id": "c1", "balance": bad }]
                }),
            )
            .await;

            let result = SimpleFinProvider::new(SimpleFinClient::default())
                .fetch(
                    &fake_source(Category::Asset, "acc_chk"),
                    Some(&credentials_for(&server)),
                )
                .await;

            assert!(matches!(result, Err(ProviderError::Other(_))), "{bad}");
        }
    }

    // Error text is written to the audit log, which must carry no
    // identifiers.
    #[tokio::test]
    async fn the_not_found_error_does_not_include_the_bridge_account_id() {
        let server = MockServer::start().await;
        mount_accounts(&server, two_account_body()).await;
        let mut source = fake_source(Category::Asset, "ACT-secret-123");
        source.id = "chase_checking".into();

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(&source, Some(&credentials_for(&server)))
            .await
            .unwrap_err();

        assert!(!err.to_string().contains("ACT-secret-123"), "got: {err}");
    }

    #[tokio::test]
    async fn a_missing_account_reports_the_bridges_own_errors() {
        let server = MockServer::start().await;
        mount_accounts(
            &server,
            json!({
                "errlist": [{ "code": "con.auth", "msg": "re-auth Chase", "conn_id": "c1" }],
                "connections": [{ "conn_id": "c1", "name": "Chase" }],
                "accounts": []
            }),
        )
        .await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();

        assert!(err.to_string().contains("re-auth Chase"), "got: {err}");
    }

    // The Bridge's developer guide says quota overruns arrive as warnings
    // in the errors array alongside valid data.
    #[tokio::test]
    async fn an_unscoped_non_auth_entry_does_not_fail_accounts_that_have_balances() {
        let server = MockServer::start().await;
        let mut body = two_account_body();
        body["errlist"] = json!([{ "code": "gen.quota", "msg": "slow down" }]);
        mount_accounts(&server, body).await;

        let result = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await;

        assert!(result.is_ok(), "got: {:?}", result.err());
    }

    #[tokio::test]
    async fn an_unscoped_auth_error_applies_to_every_account() {
        let server = MockServer::start().await;
        let mut body = two_account_body();
        body["errlist"] = json!([{ "code": "con.auth", "msg": "re-auth needed" }]);
        mount_accounts(&server, body).await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();

        assert!(matches!(err, ProviderError::Auth(_)));
    }

    #[tokio::test]
    async fn an_account_scoped_error_does_not_fail_sibling_accounts_on_the_same_connection() {
        let server = MockServer::start().await;
        mount_accounts(
            &server,
            json!({
                "errlist": [{
                    "code": "act.failed", "msg": "try later",
                    "conn_id": "c1", "account_id": "acc_chk"
                }],
                "connections": [{ "conn_id": "c1", "name": "Chase" }],
                "accounts": [
                    { "id": "acc_chk", "name": "Checking", "conn_id": "c1", "balance": "1.00" },
                    { "id": "acc_sav", "name": "Savings", "conn_id": "c1", "balance": "2.00" }
                ]
            }),
        )
        .await;

        let sibling = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_sav"),
                Some(&credentials_for(&server)),
            )
            .await;

        assert!(sibling.is_ok(), "got: {:?}", sibling.err());
    }

    #[tokio::test]
    async fn con_auth_on_this_accounts_connection_is_an_auth_error() {
        let server = MockServer::start().await;
        let mut body = two_account_body();
        body["errlist"] = json!([{ "code": "con.auth", "msg": "re-auth Chase", "conn_id": "c1" }]);
        mount_accounts(&server, body).await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }

    #[tokio::test]
    async fn a_failing_connection_does_not_fail_healthy_accounts() {
        let server = MockServer::start().await;
        let mut body = two_account_body();
        body["errlist"] = json!([{ "code": "con.auth", "msg": "re-auth Chase", "conn_id": "c1" }]);
        mount_accounts(&server, body).await;

        let accounts = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Liability, "acc_card"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap();
        assert_eq!(accounts.len(), 1);
    }

    #[tokio::test]
    async fn act_failed_on_this_account_is_a_non_auth_error() {
        let server = MockServer::start().await;
        let mut body = two_account_body();
        body["errlist"] =
            json!([{ "code": "act.failed", "msg": "try later", "account_id": "acc_chk" }]);
        mount_accounts(&server, body).await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Other(_)));
    }

    #[tokio::test]
    async fn gen_auth_applies_to_every_account() {
        let server = MockServer::start().await;
        let mut body = two_account_body();
        body["errlist"] = json!([{ "code": "gen.auth", "msg": "bad token" }]);
        mount_accounts(&server, body).await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }

    #[tokio::test]
    async fn api_error_messages_are_stripped_of_control_characters() {
        let server = MockServer::start().await;
        let mut body = two_account_body();
        body["errlist"] = json!([{ "code": "act.failed", "msg": "bad\u{1b}[31m\nnews", "account_id": "acc_chk" }]);
        mount_accounts(&server, body).await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();
        assert!(!err.to_string().chars().any(|c| c.is_control()));
    }

    #[tokio::test]
    async fn http_401_maps_to_auth() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(401))
            .mount(&server)
            .await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }

    #[tokio::test]
    async fn http_500_maps_to_transient() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(500))
            .mount(&server)
            .await;

        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(
                &fake_source(Category::Asset, "acc_chk"),
                Some(&credentials_for(&server)),
            )
            .await
            .unwrap_err();
        assert!(err.is_transient());
    }

    #[tokio::test]
    async fn connection_refused_maps_to_transient() {
        let creds = Credentials(Secret::new("http://u:p@127.0.0.1:1/simplefin".into()));
        let err = SimpleFinProvider::new(SimpleFinClient::default())
            .fetch(&fake_source(Category::Asset, "acc_chk"), Some(&creds))
            .await
            .unwrap_err();
        assert!(err.is_transient());
    }

    #[tokio::test]
    async fn sources_in_the_same_run_share_one_request() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/simplefin/accounts"))
            .respond_with(ResponseTemplate::new(200).set_body_json(two_account_body()))
            .expect(1)
            .mount(&server)
            .await;

        let provider = SimpleFinProvider::new(SimpleFinClient::default());
        let creds = credentials_for(&server);
        provider
            .fetch(&fake_source(Category::Asset, "acc_chk"), Some(&creds))
            .await
            .unwrap();
        provider
            .fetch(&fake_source(Category::Liability, "acc_card"), Some(&creds))
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn a_different_access_url_is_not_served_from_the_cache() {
        let first = MockServer::start().await;
        let second = MockServer::start().await;
        for server in [&first, &second] {
            Mock::given(method("GET"))
                .and(path("/simplefin/accounts"))
                .respond_with(ResponseTemplate::new(200).set_body_json(two_account_body()))
                .expect(1)
                .mount(server)
                .await;
        }

        let provider = SimpleFinProvider::new(SimpleFinClient::default());
        let source = fake_source(Category::Asset, "acc_chk");
        provider
            .fetch(&source, Some(&credentials_for(&first)))
            .await
            .unwrap();
        provider
            .fetch(&source, Some(&credentials_for(&second)))
            .await
            .unwrap();
    }
}
