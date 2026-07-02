//! Fetches and prints balances for an *existing* access token — no
//! Link flow, no new Item created, safe to run as many times as you
//! want against the same already-linked institution. Useful for
//! recovering an account_id you didn't capture the first time around,
//! or just checking current balances.
//!
//! Run:
//!   PLAID_CLIENT_ID=... PLAID_SECRET=... PLAID_ENVIRONMENT=production \
//!   PLAID_ACCESS_TOKEN=access-production-... \
//!     cargo run -p obol-core --example plaid_check_balances

use obol_core::{PlaidClient, PlaidConfig, PlaidEnvironment};
use secrecy::Secret;
use std::env;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client_id = env::var("PLAID_CLIENT_ID").expect("set PLAID_CLIENT_ID");
    let secret = env::var("PLAID_SECRET").expect("set PLAID_SECRET");
    let access_token = env::var("PLAID_ACCESS_TOKEN").expect("set PLAID_ACCESS_TOKEN");
    let environment = match env::var("PLAID_ENVIRONMENT").as_deref() {
        Ok("production") => PlaidEnvironment::Production,
        _ => PlaidEnvironment::Sandbox,
    };

    let client = PlaidClient::new(PlaidConfig {
        client_id,
        secret: Secret::new(secret),
        environment,
    });

    let balances = client.get_balances(&access_token).await?;
    for account in &balances.accounts {
        println!(
            "  {} ({:?}): current={:?} available={:?}  account_id={}",
            account.name,
            account.subtype,
            account.balances.current,
            account.balances.available,
            account.account_id
        );
    }

    Ok(())
}
