use serde::{Deserialize, Serialize};

use crate::account::Holding;

/// On-disk snapshot schema (spec §11.2). This is the flat, versioned DTO
/// shape snapshots are stored as — the storage layer converts `Account`
/// trait objects (see `account.rs`, D11) to/from this shape at the
/// serialization boundary.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Snapshot {
    pub schema_version: u32,
    pub snapshot_id: String,
    pub created_at: String,
    pub accounts: Vec<AccountRecord>,
}

/// The on-disk representation of one account entry (spec §11.2).
///
/// Fields are `pub(crate)`, not `pub` — construction is restricted to
/// within `obol-core` (in practice, to [`crate::pii::scrub`] and serde's
/// derived `Deserialize`), so nothing outside this crate can build one
/// with an unhashed account number or any other PII field that
/// `RawAccountData` carries but this struct doesn't expose a slot for.
/// Read access from outside the crate goes through the accessor methods
/// below.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AccountRecord {
    pub(crate) account_key: String,
    pub(crate) source_id: String,
    pub(crate) institution: String,
    pub(crate) category: Category,
    #[serde(rename = "type")]
    pub(crate) account_type: String,
    pub(crate) balance: Option<f64>,
    pub(crate) currency: String,
    pub(crate) status: Status,
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) error_message: Option<String>,
    /// This account's individual positions, if any (spec D31) — see
    /// `Account::holdings()`. Additive/optional field, same
    /// `#[serde(default)]` precedent as `error_message` above: a
    /// snapshot file written before this field existed has no
    /// `holdings` key at all and still loads fine, with this defaulting
    /// to `None`. No `CURRENT_SCHEMA_VERSION` bump needed for a purely
    /// additive optional field like this (confirmed against
    /// `migration.rs`'s migration-chain, which is reserved for breaking
    /// changes).
    #[serde(skip_serializing_if = "Option::is_none", default)]
    pub(crate) holdings: Option<Vec<Holding>>,
}

impl AccountRecord {
    pub fn account_key(&self) -> &str {
        &self.account_key
    }

    pub fn source_id(&self) -> &str {
        &self.source_id
    }

    pub fn institution(&self) -> &str {
        &self.institution
    }

    pub fn category(&self) -> Category {
        self.category
    }

    pub fn account_type(&self) -> &str {
        &self.account_type
    }

    pub fn balance(&self) -> Option<f64> {
        self.balance
    }

    pub fn currency(&self) -> &str {
        &self.currency
    }

    pub fn status(&self) -> Status {
        self.status
    }

    pub fn error_message(&self) -> Option<&str> {
        self.error_message.as_deref()
    }

    pub fn holdings(&self) -> Option<&[Holding]> {
        self.holdings.as_deref()
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Category {
    Asset,
    Liability,
    /// Forward compatibility (decision D14): a category introduced by a
    /// newer schema version than this build understands falls back here
    /// instead of failing deserialization of the whole snapshot.
    #[serde(other)]
    Unknown,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Ok,
    Error,
    /// Forward compatibility (decision D14): same rationale as
    /// `Category::Unknown`.
    #[serde(other)]
    Unknown,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ok_record() -> AccountRecord {
        AccountRecord {
            account_key: "sha256:9f2a...".into(),
            source_id: "chase_checking".into(),
            institution: "Chase".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            balance: Some(4213.55),
            currency: "USD".into(),
            status: Status::Ok,
            error_message: None,
            holdings: None,
        }
    }

    fn error_record() -> AccountRecord {
        AccountRecord {
            account_key: "sha256:71bd...".into(),
            source_id: "apple_card".into(),
            institution: "Goldman Sachs".into(),
            category: Category::Liability,
            account_type: "credit_card".into(),
            balance: None,
            currency: "USD".into(),
            status: Status::Error,
            error_message: Some("Manual entry not provided for this run".into()),
            holdings: None,
        }
    }

    fn holdings_record() -> AccountRecord {
        AccountRecord {
            account_key: "sha256:c4de...".into(),
            source_id: "vanguard_brokerage".into(),
            institution: "Vanguard".into(),
            category: Category::Asset,
            account_type: "brokerage".into(),
            balance: Some(1000.0),
            currency: "USD".into(),
            status: Status::Ok,
            error_message: None,
            holdings: Some(vec![Holding {
                symbol: "VOO".into(),
                description: "Vanguard S&P 500 ETF".into(),
                value: 1000.0,
            }]),
        }
    }

    #[test]
    fn ok_record_round_trips() {
        let record = ok_record();
        let json = serde_json::to_string(&record).unwrap();
        let back: AccountRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
    }

    #[test]
    fn error_record_round_trips() {
        let record = error_record();
        let json = serde_json::to_string(&record).unwrap();
        let back: AccountRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
    }

    #[test]
    fn holdings_record_round_trips() {
        let record = holdings_record();
        let json = serde_json::to_string(&record).unwrap();
        let back: AccountRecord = serde_json::from_str(&json).unwrap();
        assert_eq!(record, back);
    }

    #[test]
    fn a_record_with_no_holdings_omits_the_field() {
        let json = serde_json::to_string(&ok_record()).unwrap();
        assert!(!json.contains("holdings"));
    }

    #[test]
    fn a_record_with_holdings_includes_the_field() {
        let json = serde_json::to_string(&holdings_record()).unwrap();
        assert!(json.contains("holdings"));
        assert!(json.contains("VOO"));
    }

    #[test]
    fn a_record_written_before_holdings_existed_still_loads() {
        // Regression-style test, same shape as the real bug already hit
        // once this session for ProcessedFilesLedger's own
        // #[serde(default)] miss: a snapshot file from before this
        // field existed has no "holdings" key at all.
        let old_format_json = r#"{
            "account_key": "sha256:9f2a...",
            "source_id": "chase_checking",
            "institution": "Chase",
            "category": "asset",
            "type": "checking",
            "balance": 4213.55,
            "currency": "USD",
            "status": "ok"
        }"#;

        let record: AccountRecord = serde_json::from_str(old_format_json).unwrap();

        assert_eq!(record.holdings(), None);
    }

    #[test]
    fn ok_record_omits_error_message_field() {
        let json = serde_json::to_string(&ok_record()).unwrap();
        assert!(!json.contains("error_message"));
    }

    #[test]
    fn error_record_includes_error_message_field() {
        let json = serde_json::to_string(&error_record()).unwrap();
        assert!(json.contains("error_message"));
    }

    #[test]
    fn full_snapshot_round_trips() {
        let snapshot = Snapshot {
            schema_version: 1,
            snapshot_id: "b3f1-test".into(),
            created_at: "2026-06-30T09:15:00-07:00".into(),
            accounts: vec![ok_record(), error_record()],
        };
        let json = serde_json::to_string(&snapshot).unwrap();
        let back: Snapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(snapshot, back);
    }

    #[test]
    fn deserializes_the_spec_example_fixture() {
        let fixture = r#"
        {
          "schema_version": 1,
          "snapshot_id": "b3f1...(uuid)",
          "created_at": "2026-06-30T09:15:00-07:00",
          "accounts": [
            {
              "account_key": "sha256:9f2a...",
              "source_id": "chase_checking",
              "institution": "Chase",
              "category": "asset",
              "type": "checking",
              "balance": 4213.55,
              "currency": "USD",
              "status": "ok"
            },
            {
              "account_key": "sha256:71bd...",
              "source_id": "apple_card",
              "institution": "Goldman Sachs",
              "category": "liability",
              "type": "credit_card",
              "balance": null,
              "currency": "USD",
              "status": "error",
              "error_message": "Manual entry not provided for this run"
            }
          ]
        }
        "#;
        let snapshot: Snapshot = serde_json::from_str(fixture).unwrap();
        assert_eq!(snapshot.schema_version, 1);
        assert_eq!(snapshot.accounts.len(), 2);
        assert_eq!(snapshot.accounts[0].status, Status::Ok);
        assert_eq!(snapshot.accounts[0].balance, Some(4213.55));
        assert_eq!(snapshot.accounts[1].status, Status::Error);
        assert_eq!(snapshot.accounts[1].balance, None);
        assert_eq!(
            snapshot.accounts[1].error_message.as_deref(),
            Some("Manual entry not provided for this run")
        );
    }
}
