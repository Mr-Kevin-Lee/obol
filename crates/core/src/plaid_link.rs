//! Completes a Plaid Hosted Link session into real, persisted sources
//! (spec §10.1, D18, D23) — the remaining piece of task 19 once the
//! core API methods (`plaid.rs`) and the Keychain/Item-counter/
//! `sources.yaml` CRUD (tasks 9, 11, 17) existed to tie together.
//! Presenting the Link UI and letting the user pick which returned
//! accounts to track is a later, UI-layer concern (tasks 24/25) — this
//! takes that choice as an input, it doesn't make it.

use std::path::Path;

use secrecy::Secret;
use thiserror::Error;

use crate::{
    add_source, store_plaid_access_token, Category, ItemUsageCounter, KeychainError, PlaidClient,
    PlaidError, SourceConfig, SourcesError,
};

#[derive(Debug, Error)]
pub enum CompleteLinkError {
    #[error("Plaid Item limit reached (10/10) — see §7.1 for alternatives")]
    ItemLimitReached,
    #[error("Plaid API error: {0}")]
    Plaid(#[from] PlaidError),
    #[error("keychain error: {0}")]
    Keychain(#[from] KeychainError),
    #[error("sources.yaml error: {0}")]
    Sources(#[from] SourcesError),
}

/// One account the user chose to track, out of everything Plaid
/// returned for this Item (D23 — presenting that choice is a UI concern
/// this function doesn't handle, it just needs the result).
pub struct SelectedAccount {
    pub source_id: String,
    pub plaid_account_id: String,
    pub category: Category,
    pub account_type: String,
    pub institution: String,
}

/// Completes a Link session (§10.1 step 4): exchanges the
/// `public_token`, stores the access token in Keychain once per
/// selected account (all accounts under one Item share the same
/// token — a small, deliberate redundancy that keeps Keychain lookups
/// uniformly per-source rather than requiring callers to know about
/// shared Item-level credentials as a special case), increments the
/// Item counter **exactly once** regardless of how many accounts were
/// selected (§7.1 — the counter tracks Items, not accounts, per D23),
/// and writes one `sources.yaml` entry per selected account.
///
/// Checks `item_counter.is_blocked()` *first*, before any network call
/// or side effect — §7.1 requires "Connect via Plaid" to be blocked
/// entirely at 10/10, not just warned about.
pub async fn complete_plaid_link(
    client: &PlaidClient,
    public_token: &str,
    selected_accounts: Vec<SelectedAccount>,
    item_counter: &mut ItemUsageCounter,
    sources_path: &Path,
) -> Result<(), CompleteLinkError> {
    if item_counter.is_blocked() {
        return Err(CompleteLinkError::ItemLimitReached);
    }

    let exchange = client.exchange_public_token(public_token).await?;
    let access_token = Secret::new(exchange.access_token);

    for account in &selected_accounts {
        store_plaid_access_token(&account.source_id, &access_token)?;
    }

    item_counter.increment();

    for account in selected_accounts {
        let source = SourceConfig {
            id: account.source_id,
            provider: "plaid".into(),
            category: account.category,
            account_type: account.account_type,
            institution: account.institution,
            account_salt: String::new(), // overwritten by add_source (D15)
            provider_config: serde_json::json!({ "plaid_account_id": account.plaid_account_id }),
        };
        add_source(sources_path, source)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{PlaidConfig, PlaidEnvironment};
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn temp_sources_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "obol-plaid-link-test-{name}-{}.yaml",
            std::process::id()
        ))
    }

    fn dummy_client(base_url: &str) -> PlaidClient {
        PlaidClient::new(PlaidConfig {
            client_id: "test-client-id".into(),
            secret: Secret::new("test-secret".into()),
            environment: PlaidEnvironment::Custom(base_url.to_string()),
        })
    }

    #[tokio::test]
    async fn blocked_when_item_counter_is_at_the_limit_before_any_side_effect() {
        // Deliberately no wiremock server, no valid client, no cleanup
        // needed for Keychain/sources.yaml — the whole point of this
        // test is that none of that ever gets touched.
        let client = dummy_client("http://127.0.0.1:1"); // nothing listens here
        let mut counter = ItemUsageCounter::new();
        for _ in 0..10 {
            counter.increment();
        }
        let sources_path = temp_sources_path("blocked");
        let _ = std::fs::remove_file(&sources_path);

        let result = complete_plaid_link(
            &client,
            "public-sandbox-doesnt-matter",
            vec![],
            &mut counter,
            &sources_path,
        )
        .await;

        assert!(matches!(result, Err(CompleteLinkError::ItemLimitReached)));
        assert_eq!(counter.count(), 10, "count should not change on a block");
        assert!(
            !sources_path.exists(),
            "sources.yaml should never be created when blocked up front"
        );
    }

    #[tokio::test]
    #[ignore = "blocked on a parked Keychain signing issue (D24, §16) — \
                store_plaid_access_token currently fails against a real \
                Keychain regardless of signing; see keychain.rs's module \
                doc comment"]
    async fn selecting_multiple_accounts_from_one_item_increments_the_counter_once() {
        let mock_server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/item/public_token/exchange"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "access_token": "access-sandbox-test-token",
                "item_id": "item_test",
                "request_id": "req_test"
            })))
            .mount(&mock_server)
            .await;

        let client = dummy_client(&mock_server.uri());
        let mut counter = ItemUsageCounter::new();
        let sources_path = temp_sources_path("multi-account");
        let _ = std::fs::remove_file(&sources_path);

        let selected = vec![
            SelectedAccount {
                source_id: "chase_checking_test".into(),
                plaid_account_id: "acc_checking".into(),
                category: Category::Asset,
                account_type: "checking".into(),
                institution: "Chase".into(),
            },
            SelectedAccount {
                source_id: "chase_savings_test".into(),
                plaid_account_id: "acc_savings".into(),
                category: Category::Asset,
                account_type: "savings".into(),
                institution: "Chase".into(),
            },
        ];

        complete_plaid_link(
            &client,
            "public-sandbox-test",
            selected,
            &mut counter,
            &sources_path,
        )
        .await
        .expect("completion should succeed");

        assert_eq!(
            counter.count(),
            1,
            "one Item with two selected accounts should increment the counter exactly once"
        );

        let sources = crate::load_or_init(&sources_path).unwrap();
        assert_eq!(sources.len(), 2);

        crate::delete_plaid_access_token("chase_checking_test").ok();
        crate::delete_plaid_access_token("chase_savings_test").ok();
        std::fs::remove_file(&sources_path).ok();
    }
}
