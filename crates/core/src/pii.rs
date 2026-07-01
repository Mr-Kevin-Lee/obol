use sha2::{Digest, Sha256};

use crate::snapshot::{AccountRecord, Category, Status};

/// Everything a provider might have fetched about one account, before PII
/// scrubbing. These are exactly the fields spec §11.1 says must never
/// survive into a stored snapshot — providers normalize their own raw
/// response shape into this struct, then call [`scrub`].
pub struct RawAccountData {
    pub source_id: String,
    pub institution: String,
    pub category: Category,
    pub account_type: String,
    pub balance: Option<f64>,
    pub currency: String,
    pub status: Status,
    pub error_message: Option<String>,
    /// The real account number/identifier — hashed into `account_key`,
    /// never stored directly.
    pub account_number: String,
    /// Never copied into the scrubbed record.
    pub account_holder_name: Option<String>,
    /// Never copied into the scrubbed record.
    pub institution_login_id: Option<String>,
    /// Never copied into the scrubbed record — the raw provider API
    /// response, if a provider wants it around for its own pre-scrub
    /// error diagnostics.
    pub raw_response: Option<String>,
}

/// Converts a raw provider fetch result into the flat, PII-free storage
/// record (spec §11.1). This is the one place account numbers, holder
/// names, login identifiers, and raw API payloads get dropped.
pub fn scrub(raw: &RawAccountData, account_salt: &str) -> AccountRecord {
    AccountRecord {
        account_key: hash_account_number(&raw.account_number, account_salt),
        source_id: raw.source_id.clone(),
        institution: raw.institution.clone(),
        category: raw.category,
        account_type: raw.account_type.clone(),
        balance: raw.balance,
        currency: raw.currency.clone(),
        status: raw.status,
        error_message: raw.error_message.clone(),
    }
}

fn hash_account_number(account_number: &str, salt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(salt.as_bytes());
    hasher.update(account_number.as_bytes());
    let digest = hasher.finalize();
    let hex: String = digest.iter().map(|b| format!("{b:02x}")).collect();
    format!("sha256:{hex}")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn plaid_like_raw() -> RawAccountData {
        RawAccountData {
            source_id: "chase_checking".into(),
            institution: "Chase".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            balance: Some(4213.55),
            currency: "USD".into(),
            status: Status::Ok,
            error_message: None,
            account_number: "0000123456789".into(),
            account_holder_name: Some("Jane Q. Public".into()),
            institution_login_id: Some("jane.public@example.com".into()),
            raw_response: Some(
                r#"{"account_id":"abc123","mask":"6789","owners":[{"names":["Jane Q. Public"]}]}"#
                    .into(),
            ),
        }
    }

    fn webdriver_like_raw() -> RawAccountData {
        RawAccountData {
            source_id: "student_loan_navient".into(),
            institution: "Navient".into(),
            category: Category::Liability,
            account_type: "student_loan".into(),
            balance: Some(15200.00),
            currency: "USD".into(),
            status: Status::Ok,
            error_message: None,
            account_number: "9988776655".into(),
            account_holder_name: Some("Jane Q. Public".into()),
            institution_login_id: Some("jpublic".into()),
            raw_response: Some(
                "<span id=\"balance\">$15,200.00</span><span id=\"name\">Jane Q. Public</span>"
                    .into(),
            ),
        }
    }

    #[test]
    fn scrub_drops_account_holder_name() {
        for raw in [plaid_like_raw(), webdriver_like_raw()] {
            let record = scrub(&raw, "test-salt");
            let json = serde_json::to_string(&record).unwrap();
            assert!(!json.contains("Jane Q. Public"));
        }
    }

    #[test]
    fn scrub_drops_institution_login_id() {
        for raw in [plaid_like_raw(), webdriver_like_raw()] {
            let record = scrub(&raw, "test-salt");
            let json = serde_json::to_string(&record).unwrap();
            let login_id = raw.institution_login_id.as_ref().unwrap();
            assert!(!json.contains(login_id.as_str()));
        }
    }

    #[test]
    fn scrub_never_carries_the_raw_account_number_forward() {
        for raw in [plaid_like_raw(), webdriver_like_raw()] {
            let record = scrub(&raw, "test-salt");
            let json = serde_json::to_string(&record).unwrap();
            assert!(!json.contains(raw.account_number.as_str()));
            assert!(!record.account_key.contains(raw.account_number.as_str()));
        }
    }

    #[test]
    fn scrub_drops_the_raw_response_payload() {
        for raw in [plaid_like_raw(), webdriver_like_raw()] {
            let record = scrub(&raw, "test-salt");
            let json = serde_json::to_string(&record).unwrap();
            // AccountRecord has no field for raw_response at all, so this
            // is really a structural guarantee — this test just documents
            // and locks in that guarantee against future field additions.
            assert!(!json.contains("mask"));
            assert!(!json.contains("balance\">$15,200"));
        }
    }

    #[test]
    fn same_account_number_and_salt_produce_the_same_key_across_runs() {
        let raw = plaid_like_raw();
        let first = scrub(&raw, "stable-salt");
        let second = scrub(&raw, "stable-salt");
        assert_eq!(first.account_key, second.account_key);
    }

    #[test]
    fn different_salts_produce_different_keys_for_the_same_account_number() {
        let raw = plaid_like_raw();
        let a = scrub(&raw, "salt-a");
        let b = scrub(&raw, "salt-b");
        assert_ne!(a.account_key, b.account_key);
    }

    #[test]
    fn account_key_is_prefixed_and_hex_encoded() {
        let record = scrub(&plaid_like_raw(), "test-salt");
        let hex_part = record.account_key.strip_prefix("sha256:").unwrap();
        assert_eq!(hex_part.len(), 64); // SHA-256 -> 32 bytes -> 64 hex chars
        assert!(hex_part.chars().all(|c| c.is_ascii_hexdigit()));
    }
}
