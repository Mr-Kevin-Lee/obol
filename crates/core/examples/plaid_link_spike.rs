//! Manual validation harness for the Plaid Hosted Link flow (spec §10.1,
//! decision D18). This is the core Plaid-API interaction only — Keychain
//! storage, the Item usage counter, and writing a `sources.yaml` entry are
//! separate tasks (17, 9, 11) not wired in here yet. This exists to prove
//! out linking a real institution end-to-end before those pieces exist.
//!
//! **Run against Sandbox first** (default, unlimited free relinking) to
//! confirm the polling/exchange mechanics actually work, before ever
//! pointing this at Production — a real Item is a lifetime-limited
//! resource (§7), not something to spend on debugging.
//!
//! Sandbox:
//!   PLAID_CLIENT_ID=... PLAID_SECRET=... \
//!     cargo run --example plaid_link_spike
//!
//! Production (only after Sandbox has confirmed this works):
//!   PLAID_CLIENT_ID=... PLAID_SECRET=... PLAID_ENVIRONMENT=production \
//!     cargo run --example plaid_link_spike

use obol_core::{PlaidClient, PlaidConfig, PlaidEnvironment};
use secrecy::Secret;
use std::env;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let client_id = env::var("PLAID_CLIENT_ID").expect("set PLAID_CLIENT_ID");
    let secret = env::var("PLAID_SECRET").expect("set PLAID_SECRET");
    let environment = match env::var("PLAID_ENVIRONMENT").as_deref() {
        Ok("production") => PlaidEnvironment::Production,
        _ => PlaidEnvironment::Sandbox,
    };

    println!(
        "Environment: {}",
        match environment {
            PlaidEnvironment::Sandbox => "Sandbox",
            PlaidEnvironment::Production => "PRODUCTION — this will link a real institution",
        }
    );

    let client = PlaidClient::new(PlaidConfig {
        client_id,
        secret: Secret::new(secret),
        environment,
    });

    println!("Creating Link token (Hosted Link)...");
    let link = client.create_link_token("obol-single-user", "Obol").await?;
    println!("link_token: {}", link.link_token);
    println!("expiration:  {}", link.expiration);

    match &link.hosted_link_url {
        Some(url) => println!("\nOpen this URL in your browser to complete Link:\n  {url}\n"),
        None => println!(
            "\nNo hosted_link_url field came back on this response. Printing what we do \
             have above — check Plaid's current docs for the field name if this keeps \
             happening, and tell me so I can fix create_link_token().\n"
        ),
    }

    println!("Polling /link/token/get every 5s — complete the flow in your browser now.");
    println!("Press Ctrl+C to stop.\n");

    loop {
        tokio::time::sleep(Duration::from_secs(5)).await;
        let status = client.get_link_token_status(&link.link_token).await?;

        let Some(session) = status.link_sessions.iter().find(|s| s.is_finished()) else {
            println!(
                "Still waiting... ({} session(s) so far, none finished)",
                status.link_sessions.len()
            );
            continue;
        };

        let Some(public_token) = session.public_token() else {
            println!(
                "\nSession finished but no public_token — the flow was abandoned or hit \
                 an error. Full event trail:\n{}",
                serde_json::to_string_pretty(&session.events).unwrap_or_default()
            );
            return Ok(());
        };

        println!("\nLink session complete. public_token: {public_token}");

        let exchange = client.exchange_public_token(public_token).await?;
        println!(
            "Exchanged for access_token (item_id: {}). Not persisted anywhere by this \
             spike — Keychain storage is task 17, not wired in yet.",
            exchange.item_id
        );

        println!("\nFetching balances to confirm the whole loop works end to end...");
        let balances = client.get_balances(&exchange.access_token).await?;
        for account in &balances.accounts {
            println!(
                "  {} ({:?}): current={:?} available={:?}",
                account.name, account.subtype, account.balances.current, account.balances.available
            );
        }

        println!("\nDone.");
        break;
    }

    Ok(())
}
