//! Snapshot builders shared by this crate's tests.
//!
//! Built through serde rather than struct literals because
//! `AccountRecord`'s fields are `pub(crate)` to `obol-core`: construction
//! is restricted to inside core so nothing outside it can build a record
//! holding un-scrubbed PII (§11.1). Going through the on-disk JSON shape is
//! the supported route, and doubles as a check that this crate still reads
//! real snapshot files.

use obol_core::Snapshot;
use serde_json::{json, Value};

pub fn snapshot_at(created_at: &str, accounts: Value) -> Snapshot {
    serde_json::from_value(json!({
        "schema_version": 1,
        "snapshot_id": "test-snapshot",
        "created_at": created_at,
        "accounts": accounts,
    }))
    .expect("fixture snapshot should deserialize")
}

pub fn asset(source_id: &str, balance: f64) -> Value {
    json!({
        "account_key": format!("sha256:{source_id}"),
        "source_id": source_id,
        "institution": "Chase",
        "category": "asset",
        "type": "checking",
        "balance": balance,
        "currency": "USD",
        "status": "ok",
    })
}

pub fn liability(source_id: &str, balance: f64) -> Value {
    json!({
        "account_key": format!("sha256:{source_id}"),
        "source_id": source_id,
        "institution": "Goldman Sachs",
        "category": "liability",
        "type": "credit_card",
        "balance": balance,
        "currency": "USD",
        "status": "ok",
    })
}
