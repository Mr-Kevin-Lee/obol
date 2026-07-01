//! Deliberately triggers real Plaid API error responses, to validate
//! `PlaidError` handling against real data instead of assumption — same
//! "confirm it against reality" approach as the rest of this client.
//! Safe to run: every probe uses an intentionally invalid token, never
//! real institution data.
//!
//! Run:
//!   PLAID_CLIENT_ID=... PLAID_SECRET=... \
//!   PLAID_CAPTURE_FIXTURES_DIR=crates/core/tests/fixtures/plaid \
//!     cargo run -p obol-core --example plaid_error_probe

use obol_core::{PlaidClient, PlaidConfig, PlaidEnvironment};
use secrecy::Secret;
use std::env;

#[tokio::main]
async fn main() {
    let client_id = env::var("PLAID_CLIENT_ID").expect("set PLAID_CLIENT_ID");
    let secret = env::var("PLAID_SECRET").expect("set PLAID_SECRET");

    let client = PlaidClient::new(PlaidConfig {
        client_id: client_id.clone(),
        secret: Secret::new(secret.clone()),
        environment: PlaidEnvironment::Sandbox,
    });

    println!("--- Probe 1: exchange an obviously-invalid public_token ---");
    match client
        .exchange_public_token("public-sandbox-not-a-real-token")
        .await
    {
        Ok(r) => println!("Unexpected success: {r:?}\n"),
        Err(e) => println!("Got error (expected): {e}\n  debug: {e:?}\n"),
    }

    println!("--- Probe 2: wrong secret entirely (auth failure) ---");
    let bad_secret_client = PlaidClient::new(PlaidConfig {
        client_id,
        secret: Secret::new("definitely-not-the-real-secret".to_string()),
        environment: PlaidEnvironment::Sandbox,
    });
    match bad_secret_client
        .get_balances("access-sandbox-not-a-real-token")
        .await
    {
        Ok(r) => println!("Unexpected success: {r:?}\n"),
        Err(e) => println!("Got error (expected): {e}\n  debug: {e:?}\n"),
    }

    println!("--- Probe 3: fetch balances with a garbage access_token (correct secret) ---");
    match client.get_balances("access-sandbox-not-a-real-token").await {
        Ok(r) => println!("Unexpected success: {r:?}\n"),
        Err(e) => println!("Got error (expected): {e}\n  debug: {e:?}\n"),
    }

    println!("Done. Check for [fixture captured] lines above for the *_error.json files.");
}
