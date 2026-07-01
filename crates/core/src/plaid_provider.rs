//! Ties the hand-rolled Plaid REST client (`plaid.rs`) into the
//! `Provider` trait (§10). A `Source` is one Plaid account, not one
//! Item/login (decision D23) — `fetch()` calls Balance once per Item and
//! filters the response down to the one account matching this source's
//! `plaid_account_id`.

use async_trait::async_trait;
use secrecy::ExposeSecret;

use crate::pii::hash_account_number;
use crate::plaid::{PlaidClient, PlaidError};
use crate::{Account, AccountStatus, Asset, Category, Credentials, Liability};
use crate::{Provider, ProviderError, SourceConfig};

pub struct PlaidProvider {
    client: PlaidClient,
}

impl PlaidProvider {
    pub fn new(client: PlaidClient) -> Self {
        Self { client }
    }
}

#[async_trait]
impl Provider for PlaidProvider {
    async fn fetch(
        &self,
        source: &SourceConfig,
        credentials: Option<&Credentials>,
    ) -> Result<Vec<Box<dyn Account>>, ProviderError> {
        let access_token = credentials
            .ok_or_else(|| ProviderError::Auth("Plaid source has no access token".into()))?
            .0
            .expose_secret();

        let plaid_account_id = source
            .provider_config
            .get("plaid_account_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::Other(format!(
                    "source {} is missing provider_config.plaid_account_id (D23)",
                    source.id
                ))
            })?;

        let response = self
            .client
            .get_balances(access_token)
            .await
            .map_err(map_plaid_error)?;

        let plaid_account = response
            .accounts
            .into_iter()
            .find(|a| a.account_id == plaid_account_id)
            .ok_or_else(|| {
                ProviderError::Other(format!(
                    "account {plaid_account_id} not found in Plaid's response for source {} \
                     — the Item may have changed shape since this source was added",
                    source.id
                ))
            })?;

        let account_key = hash_account_number(&plaid_account.account_id, &source.account_salt);
        let balance = plaid_account.balances.current;

        let account: Box<dyn Account> = match source.category {
            Category::Asset => Box::new(Asset {
                account_key,
                institution: source.institution.clone(),
                r#type: source.account_type.clone(),
                balance,
                status: AccountStatus::Ok,
            }),
            Category::Liability => Box::new(Liability {
                account_key,
                institution: source.institution.clone(),
                r#type: source.account_type.clone(),
                balance,
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

/// Maps a Plaid API error to the auth/transient/other distinction §9's
/// retry policy needs. The `INVALID_INPUT` mapping is confirmed against
/// real Plaid error responses (task 16/19 testing: INVALID_PUBLIC_TOKEN,
/// INVALID_API_KEYS, INVALID_ACCESS_TOKEN all came back this way — none
/// of those are fixed by retrying). The other `error_type` categories
/// below are reasonable, commonly-documented Plaid conventions, not
/// something we've triggered and observed directly — treat them as a
/// sensible default, not a verified mapping, and revisit if real usage
/// disagrees. Unrecognized shapes default to `Other` (not retried)
/// rather than `Transient`, since retrying something we don't understand
/// risks compounding the problem (e.g. against Plaid's rate limits) more
/// than just surfacing the failure once does.
fn map_plaid_error(err: PlaidError) -> ProviderError {
    match err {
        PlaidError::Transport(e) => ProviderError::Transient(e.to_string()),
        PlaidError::Api {
            error_type,
            error_code,
            error_message,
        } => {
            let is_auth = matches!(error_type.as_str(), "INVALID_INPUT" | "ITEM_ERROR");
            let is_transient = matches!(
                error_type.as_str(),
                "RATE_LIMIT_EXCEEDED" | "API_ERROR" | "INSTITUTION_ERROR"
            );
            let detail = format!("[{error_type}/{error_code}] {error_message}");
            if is_auth {
                ProviderError::Auth(detail)
            } else if is_transient {
                ProviderError::Transient(detail)
            } else {
                ProviderError::Other(detail)
            }
        }
        PlaidError::UnexpectedErrorResponse { status, raw_body } => ProviderError::Other(format!(
            "unexpected Plaid response (HTTP {status}): {raw_body}"
        )),
        PlaidError::Parse(e) => {
            ProviderError::Other(format!("failed to parse Plaid response: {e}"))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PlaidConfig, PlaidEnvironment};
    use secrecy::Secret;
    use serde_json::json;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    async fn provider_against(mock_server: &MockServer) -> PlaidProvider {
        let client = PlaidClient::new(PlaidConfig {
            client_id: "test-client-id".into(),
            secret: Secret::new("test-secret".into()),
            environment: PlaidEnvironment::Custom(mock_server.uri()),
        });
        PlaidProvider::new(client)
    }

    fn fake_source(category: Category, plaid_account_id: &str) -> SourceConfig {
        SourceConfig {
            id: "chase_checking".into(),
            provider: "plaid".into(),
            category,
            account_type: "checking".into(),
            institution: "Chase".into(),
            account_salt: "test-salt".into(),
            provider_config: json!({ "plaid_account_id": plaid_account_id }),
        }
    }

    fn fake_credentials() -> Credentials {
        Credentials(Secret::new("access-sandbox-fake-token".into()))
    }

    #[tokio::test]
    async fn fetch_returns_the_matching_account_as_an_asset() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/accounts/balance/get"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "accounts": [
                    {
                        "account_id": "acc_checking",
                        "balances": { "available": 100.0, "current": 110.0, "limit": null, "iso_currency_code": "USD" },
                        "mask": "0000",
                        "name": "Plaid Checking",
                        "official_name": null,
                        "subtype": "checking",
                        "type": "depository"
                    },
                    {
                        "account_id": "acc_savings",
                        "balances": { "available": 500.0, "current": 500.0, "limit": null, "iso_currency_code": "USD" },
                        "mask": "1111",
                        "name": "Plaid Saving",
                        "official_name": null,
                        "subtype": "savings",
                        "type": "depository"
                    }
                ],
                "item": { "item_id": "item_1", "institution_id": "ins_56" },
                "request_id": "req_1"
            })))
            .mount(&mock_server)
            .await;

        let provider = provider_against(&mock_server).await;
        let source = fake_source(Category::Asset, "acc_checking");
        let creds = fake_credentials();

        let accounts = provider.fetch(&source, Some(&creds)).await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].balance(), Some(110.0));
        assert_eq!(accounts[0].institution(), "Chase");
    }

    #[tokio::test]
    async fn fetch_builds_a_liability_when_source_category_is_liability() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/accounts/balance/get"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "accounts": [
                    {
                        "account_id": "acc_credit_card",
                        "balances": { "available": null, "current": 250.0, "limit": 5000.0, "iso_currency_code": "USD" },
                        "mask": "9999",
                        "name": "Plaid Credit Card",
                        "official_name": null,
                        "subtype": "credit card",
                        "type": "credit"
                    }
                ],
                "item": { "item_id": "item_1", "institution_id": "ins_56" },
                "request_id": "req_2"
            })))
            .mount(&mock_server)
            .await;

        let provider = provider_against(&mock_server).await;
        let source = fake_source(Category::Liability, "acc_credit_card");
        let creds = fake_credentials();

        let accounts = provider.fetch(&source, Some(&creds)).await.unwrap();
        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].net_worth_contribution(), -250.0);
    }

    #[tokio::test]
    async fn fetch_errors_when_account_id_is_not_in_the_response() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/accounts/balance/get"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "accounts": [],
                "item": { "item_id": "item_1", "institution_id": "ins_56" },
                "request_id": "req_3"
            })))
            .mount(&mock_server)
            .await;

        let provider = provider_against(&mock_server).await;
        let source = fake_source(Category::Asset, "acc_does_not_exist");
        let creds = fake_credentials();

        let err = provider.fetch(&source, Some(&creds)).await.unwrap_err();
        assert!(!err.is_transient());
    }

    #[tokio::test]
    async fn fetch_without_credentials_is_an_auth_error() {
        let mock_server = MockServer::start().await;
        let provider = provider_against(&mock_server).await;
        let source = fake_source(Category::Asset, "acc_checking");

        let err = provider.fetch(&source, None).await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
    }

    #[tokio::test]
    async fn invalid_access_token_error_maps_to_auth_not_transient() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/accounts/balance/get"))
            .respond_with(ResponseTemplate::new(400).set_body_json(json!({
                "error_type": "INVALID_INPUT",
                "error_code": "INVALID_ACCESS_TOKEN",
                "error_message": "provided access token is in an invalid format"
            })))
            .mount(&mock_server)
            .await;

        let provider = provider_against(&mock_server).await;
        let source = fake_source(Category::Asset, "acc_checking");
        let creds = fake_credentials();

        let err = provider.fetch(&source, Some(&creds)).await.unwrap_err();
        assert!(matches!(err, ProviderError::Auth(_)));
        assert!(!err.is_transient());
    }

    #[test]
    fn map_plaid_error_treats_unrecognized_error_types_as_other_not_transient() {
        let err = map_plaid_error(PlaidError::Api {
            error_type: "SOME_FUTURE_ERROR_TYPE".into(),
            error_code: "SOMETHING".into(),
            error_message: "unrecognized".into(),
        });
        assert!(matches!(err, ProviderError::Other(_)));
        assert!(!err.is_transient());
    }
}
