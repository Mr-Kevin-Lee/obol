//! Hand-rolled Plaid REST client (spec §5, §7, §14, D6). Covers the
//! Balance product only, plus Link/exchange plumbing. Confirmed via real
//! Sandbox testing (decision D22) that Balance alone returns usable
//! account-level current balances for investment- and liability-type
//! accounts too — this project doesn't need the Investments or
//! Liabilities products at all for a net-worth-only use case, which
//! keeps both cost and client complexity down to one endpoint.

use secrecy::{ExposeSecret, Secret};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use thiserror::Error;

const SANDBOX_BASE_URL: &str = "https://sandbox.plaid.com";
const PRODUCTION_BASE_URL: &str = "https://production.plaid.com";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlaidEnvironment {
    Sandbox,
    Production,
}

impl PlaidEnvironment {
    fn base_url(self) -> &'static str {
        match self {
            PlaidEnvironment::Sandbox => SANDBOX_BASE_URL,
            PlaidEnvironment::Production => PRODUCTION_BASE_URL,
        }
    }
}

#[derive(Debug, Error)]
pub enum PlaidError {
    #[error("network/transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("Plaid API error [{error_type}/{error_code}]: {error_message}")]
    Api {
        error_type: String,
        error_code: String,
        error_message: String,
    },
    #[error("failed to parse Plaid response: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Credentials for this app's own Plaid API access (not a user's bank
/// credential — this is our client_id/secret pair with Plaid). `secret`
/// is wrapped so it can't leak via an accidental `{:?}` (spec §4's
/// `secrecy` convention).
pub struct PlaidConfig {
    pub client_id: String,
    pub secret: Secret<String>,
    pub environment: PlaidEnvironment,
}

pub struct PlaidClient {
    http: reqwest::Client,
    config: PlaidConfig,
}

#[derive(Serialize)]
struct WithAuth<'a, T: Serialize> {
    client_id: &'a str,
    secret: &'a str,
    #[serde(flatten)]
    inner: T,
}

#[derive(Debug, Deserialize)]
struct PlaidApiErrorBody {
    error_type: String,
    error_code: String,
    error_message: String,
}

impl PlaidClient {
    pub fn new(config: PlaidConfig) -> Self {
        Self {
            http: reqwest::Client::new(),
            config,
        }
    }

    async fn post<Req: Serialize, Resp: DeserializeOwned>(
        &self,
        path: &str,
        body: Req,
    ) -> Result<Resp, PlaidError> {
        let wrapped = WithAuth {
            client_id: &self.config.client_id,
            secret: self.config.secret.expose_secret(),
            inner: body,
        };
        let url = format!("{}{}", self.config.environment.base_url(), path);
        let response = self.http.post(&url).json(&wrapped).send().await?;
        let status = response.status();
        let text = response.text().await?;

        if !status.is_success() {
            let error: PlaidApiErrorBody = serde_json::from_str(&text)?;
            return Err(PlaidError::Api {
                error_type: error.error_type,
                error_code: error.error_code,
                error_message: error.error_message,
            });
        }

        Ok(serde_json::from_str(&text)?)
    }

    /// Sandbox-only: mints a `public_token` without a real Link/browser
    /// flow, so Sandbox integration tests don't need a human clicking
    /// through Hosted Link. Not available in Production.
    pub async fn sandbox_create_public_token(
        &self,
        institution_id: &str,
        initial_products: &[&str],
    ) -> Result<SandboxPublicTokenResponse, PlaidError> {
        #[derive(Serialize)]
        struct Req<'a> {
            institution_id: &'a str,
            initial_products: &'a [&'a str],
        }
        self.post(
            "/sandbox/public_token/create",
            Req {
                institution_id,
                initial_products,
            },
        )
        .await
    }

    pub async fn exchange_public_token(
        &self,
        public_token: &str,
    ) -> Result<ExchangeResponse, PlaidError> {
        #[derive(Serialize)]
        struct Req<'a> {
            public_token: &'a str,
        }
        self.post("/item/public_token/exchange", Req { public_token })
            .await
    }

    pub async fn get_balances(&self, access_token: &str) -> Result<BalanceResponse, PlaidError> {
        #[derive(Serialize)]
        struct Req<'a> {
            access_token: &'a str,
        }
        self.post("/accounts/balance/get", Req { access_token })
            .await
    }

    pub async fn remove_item(&self, access_token: &str) -> Result<RemoveItemResponse, PlaidError> {
        #[derive(Serialize)]
        struct Req<'a> {
            access_token: &'a str,
        }
        self.post("/item/remove", Req { access_token }).await
    }
}

#[derive(Debug, Deserialize)]
pub struct SandboxPublicTokenResponse {
    pub public_token: String,
    pub request_id: String,
}

#[derive(Debug, Deserialize)]
pub struct ExchangeResponse {
    pub access_token: String,
    pub item_id: String,
    pub request_id: String,
}

#[derive(Debug, Deserialize)]
pub struct RemoveItemResponse {
    pub request_id: String,
}

#[derive(Debug, Deserialize)]
pub struct PlaidAccount {
    pub account_id: String,
    pub balances: PlaidBalances,
    pub mask: Option<String>,
    pub name: String,
    pub official_name: Option<String>,
    pub subtype: Option<String>,
    #[serde(rename = "type")]
    pub account_type: String,
}

#[derive(Debug, Deserialize)]
pub struct PlaidBalances {
    pub available: Option<f64>,
    pub current: Option<f64>,
    pub limit: Option<f64>,
    pub iso_currency_code: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PlaidItem {
    pub item_id: String,
    pub institution_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BalanceResponse {
    pub accounts: Vec<PlaidAccount>,
    pub item: PlaidItem,
    pub request_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Reads Sandbox credentials from the environment. Only called by
    /// `#[ignore]`d tests, which you run explicitly once you have keys:
    ///   PLAID_SANDBOX_CLIENT_ID=... PLAID_SANDBOX_SECRET=... \
    ///     cargo test -p obol-core -- --ignored
    fn sandbox_client_from_env() -> PlaidClient {
        let client_id = std::env::var("PLAID_SANDBOX_CLIENT_ID")
            .expect("PLAID_SANDBOX_CLIENT_ID must be set to run Plaid Sandbox tests");
        let secret = std::env::var("PLAID_SANDBOX_SECRET")
            .expect("PLAID_SANDBOX_SECRET must be set to run Plaid Sandbox tests");
        PlaidClient::new(PlaidConfig {
            client_id,
            secret: Secret::new(secret),
            environment: PlaidEnvironment::Sandbox,
        })
    }

    // First Platypus Bank — Plaid's general-purpose Sandbox test
    // institution. Confirmed working here for Balance against accounts
    // created under the depository, investments, and liabilities product
    // scopes alike.
    const SANDBOX_INSTITUTION_ID: &str = "ins_109508";

    #[tokio::test]
    #[ignore = "requires PLAID_SANDBOX_CLIENT_ID/PLAID_SANDBOX_SECRET and network access"]
    async fn sandbox_balance_flow_end_to_end() {
        let client = sandbox_client_from_env();
        let public_token = client
            .sandbox_create_public_token(SANDBOX_INSTITUTION_ID, &["auth"])
            .await
            .expect("sandbox_create_public_token failed")
            .public_token;
        let exchange = client
            .exchange_public_token(&public_token)
            .await
            .expect("exchange_public_token failed");
        let balances = client
            .get_balances(&exchange.access_token)
            .await
            .expect("get_balances failed");
        assert!(
            !balances.accounts.is_empty(),
            "expected at least one account back from Sandbox"
        );
    }

    #[tokio::test]
    #[ignore = "requires PLAID_SANDBOX_CLIENT_ID/PLAID_SANDBOX_SECRET and network access; \
                confirms Balance alone works on an investments-type account, without \
                the Investments product enabled (decision D22)"]
    async fn sandbox_balance_works_on_investment_account() {
        let client = sandbox_client_from_env();
        let public_token = client
            .sandbox_create_public_token(SANDBOX_INSTITUTION_ID, &["investments"])
            .await
            .expect("sandbox_create_public_token failed")
            .public_token;
        let exchange = client
            .exchange_public_token(&public_token)
            .await
            .expect("exchange_public_token failed");
        let balances = client
            .get_balances(&exchange.access_token)
            .await
            .expect("get_balances failed on an investments-only Item");
        assert!(!balances.accounts.is_empty());
        let has_a_balance = balances
            .accounts
            .iter()
            .any(|a| a.balances.current.is_some());
        assert!(
            has_a_balance,
            "expected at least one account with a current balance"
        );
    }

    #[tokio::test]
    #[ignore = "requires PLAID_SANDBOX_CLIENT_ID/PLAID_SANDBOX_SECRET and network access; \
                confirms Balance alone works on a liabilities-type account, without \
                the Liabilities product enabled (decision D22)"]
    async fn sandbox_balance_works_on_liability_account() {
        let client = sandbox_client_from_env();
        let public_token = client
            .sandbox_create_public_token(SANDBOX_INSTITUTION_ID, &["liabilities"])
            .await
            .expect("sandbox_create_public_token failed")
            .public_token;
        let exchange = client
            .exchange_public_token(&public_token)
            .await
            .expect("exchange_public_token failed");
        let balances = client
            .get_balances(&exchange.access_token)
            .await
            .expect("get_balances failed on a liabilities-only Item");
        assert!(!balances.accounts.is_empty());
        let has_a_balance = balances
            .accounts
            .iter()
            .any(|a| a.balances.current.is_some());
        assert!(
            has_a_balance,
            "expected at least one account with a current balance"
        );
    }
}
