//! `merchant_rules.yaml` (spec §11.4, D44): merchant key → category, the
//! first and strongest layer of categorization. Written by an edit in the
//! app and seeded by the one-time Simplifi import.
//!
//! **Contract (task 111b), implemented by task 114.** A separate file from
//! `rules.yaml` on purpose: adding a section there would mean editing
//! `rules_storage.rs`, which #37 changes. Writes go through `atomic_write`.

use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::atomic_write::write_atomically;

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct MerchantRules {
    /// Merchant key → category.
    #[serde(default)]
    pub rules: BTreeMap<String, String>,
}

#[derive(Debug, Error)]
pub enum MerchantRulesError {
    #[error("merchant_rules.yaml could not be parsed: {0}")]
    Parse(serde_saphyr::Error),
    #[error("failed to write merchant_rules.yaml: {0}")]
    Serialize(serde_saphyr::ser_error::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Loads `merchant_rules.yaml`, creating an empty one if it doesn't exist.
pub fn load_or_init_merchant_rules(path: &Path) -> Result<MerchantRules, MerchantRulesError> {
    if !path.exists() {
        let empty = MerchantRules::default();
        save(path, &empty)?;
        return Ok(empty);
    }
    let contents = fs::read_to_string(path)?;
    serde_saphyr::from_str(&contents).map_err(MerchantRulesError::Parse)
}

/// Sets one merchant's category (read, modify, write) and returns the
/// updated rules. Overwrites any existing rule for that merchant.
///
/// `merchant_key` is stored exactly as given: callers pass the
/// already-normalized key from a categorized row.
pub fn set_merchant_category(
    path: &Path,
    merchant_key: &str,
    category: &str,
) -> Result<MerchantRules, MerchantRulesError> {
    let mut merchant_rules = load_or_init_merchant_rules(path)?;
    merchant_rules
        .rules
        .insert(merchant_key.to_string(), category.to_string());
    save(path, &merchant_rules)?;
    Ok(merchant_rules)
}

/// Adds a rule for each seeded merchant that has none, returning how many
/// were added. An existing rule — an edit made in the app — always wins.
pub fn seed_merchant_rules(
    path: &Path,
    seeds: &BTreeMap<String, String>,
) -> Result<usize, MerchantRulesError> {
    let mut merchant_rules = load_or_init_merchant_rules(path)?;
    let before = merchant_rules.rules.len();
    for (merchant_key, category) in seeds {
        merchant_rules
            .rules
            .entry(merchant_key.clone())
            .or_insert_with(|| category.clone());
    }
    let added = merchant_rules.rules.len() - before;
    if added > 0 {
        save(path, &merchant_rules)?;
    }
    Ok(added)
}

fn save(path: &Path, merchant_rules: &MerchantRules) -> Result<(), MerchantRulesError> {
    let yaml = serde_saphyr::to_string(merchant_rules).map_err(MerchantRulesError::Serialize)?;
    Ok(write_atomically(path, yaml.as_bytes())?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const MALFORMED: &str = "rules: [this is not valid: yaml: at all: -";

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "obol-merchant-rules-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("merchant_rules.yaml")
    }

    fn category_of(rules: &MerchantRules, merchant_key: &str) -> Option<String> {
        rules.rules.get(merchant_key).cloned()
    }

    #[test]
    fn should_create_the_file_when_missing() {
        let path = temp_path("init-creates");

        load_or_init_merchant_rules(&path).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn should_write_the_file_with_0600_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let path = temp_path("perms");

        set_merchant_category(&path, "AMAZON", "Shopping").unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn should_start_empty_when_missing() {
        let path = temp_path("init-empty");

        let rules = load_or_init_merchant_rules(&path).unwrap();

        assert_eq!(rules, MerchantRules::default());
    }

    #[test]
    fn should_reload_an_initialized_file_as_empty() {
        let path = temp_path("init-reload");
        load_or_init_merchant_rules(&path).unwrap();

        let rules = load_or_init_merchant_rules(&path).unwrap();

        assert_eq!(rules, MerchantRules::default());
    }

    #[test]
    fn should_round_trip_a_set_rule_through_the_file() {
        let path = temp_path("round-trip");
        set_merchant_category(&path, "AMAZON", "Shopping").unwrap();

        let rules = load_or_init_merchant_rules(&path).unwrap();

        assert_eq!(category_of(&rules, "AMAZON").as_deref(), Some("Shopping"));
    }

    #[test]
    fn should_return_the_updated_rules_from_set() {
        let path = temp_path("set-returns");

        let rules = set_merchant_category(&path, "AMAZON", "Shopping").unwrap();

        assert_eq!(category_of(&rules, "AMAZON").as_deref(), Some("Shopping"));
    }

    #[test]
    fn should_overwrite_an_existing_rule() {
        let path = temp_path("overwrite");
        set_merchant_category(&path, "AMAZON", "Shopping").unwrap();
        set_merchant_category(&path, "AMAZON", "Gifts").unwrap();

        let rules = load_or_init_merchant_rules(&path).unwrap();

        assert_eq!(category_of(&rules, "AMAZON").as_deref(), Some("Gifts"));
    }

    #[test]
    fn should_keep_rules_for_different_merchants() {
        let path = temp_path("coexist");
        set_merchant_category(&path, "AMAZON", "Shopping").unwrap();
        set_merchant_category(&path, "COSTCO", "Groceries").unwrap();

        let rules = load_or_init_merchant_rules(&path).unwrap();

        assert_eq!(rules.rules.len(), 2);
    }

    #[test]
    fn should_store_the_key_exactly_as_given() {
        let path = temp_path("exact-key");

        let rules = set_merchant_category(&path, "amazon ", "Shopping").unwrap();

        assert_eq!(category_of(&rules, "amazon ").as_deref(), Some("Shopping"));
    }

    #[test]
    fn should_return_a_parse_error_for_malformed_yaml() {
        let path = temp_path("malformed");
        fs::write(&path, MALFORMED).unwrap();

        let err = load_or_init_merchant_rules(&path).unwrap_err();

        assert!(matches!(err, MerchantRulesError::Parse(_)));
    }

    #[test]
    fn should_return_a_parse_error_on_set_for_malformed_yaml() {
        let path = temp_path("malformed-set-err");
        fs::write(&path, MALFORMED).unwrap();

        let err = set_merchant_category(&path, "AMAZON", "Shopping").unwrap_err();

        assert!(matches!(err, MerchantRulesError::Parse(_)));
    }

    #[test]
    fn should_leave_a_malformed_file_untouched_on_set() {
        let path = temp_path("malformed-set");
        fs::write(&path, MALFORMED).unwrap();

        let _ = set_merchant_category(&path, "AMAZON", "Shopping");

        assert_eq!(fs::read_to_string(&path).unwrap(), MALFORMED);
    }

    fn seeds(pairs: &[(&str, &str)]) -> BTreeMap<String, String> {
        pairs
            .iter()
            .map(|(key, category)| (key.to_string(), category.to_string()))
            .collect()
    }

    #[test]
    fn should_seed_a_rule_for_a_new_merchant() {
        let path = temp_path("seed-new");

        seed_merchant_rules(&path, &seeds(&[("COSTCO", "Groceries")])).unwrap();

        let rules = load_or_init_merchant_rules(&path).unwrap();
        assert_eq!(category_of(&rules, "COSTCO").as_deref(), Some("Groceries"));
    }

    #[test]
    fn should_not_overwrite_an_existing_rule_when_seeding() {
        let path = temp_path("seed-keep");
        set_merchant_category(&path, "AMAZON", "Gifts").unwrap();

        // A new merchant alongside, so the file is rewritten.
        seed_merchant_rules(
            &path,
            &seeds(&[("AMAZON", "Shopping"), ("COSTCO", "Groceries")]),
        )
        .unwrap();

        let rules = load_or_init_merchant_rules(&path).unwrap();
        assert_eq!(category_of(&rules, "AMAZON").as_deref(), Some("Gifts"));
    }

    #[test]
    fn should_count_only_the_rules_seeding_added() {
        let path = temp_path("seed-count");
        set_merchant_category(&path, "AMAZON", "Gifts").unwrap();

        let added = seed_merchant_rules(
            &path,
            &seeds(&[("AMAZON", "Shopping"), ("COSTCO", "Groceries")]),
        )
        .unwrap();

        assert_eq!(added, 1);
    }

    #[test]
    fn should_leave_a_malformed_file_untouched_on_seed() {
        let path = temp_path("seed-malformed");
        fs::write(&path, MALFORMED).unwrap();

        let _ = seed_merchant_rules(&path, &seeds(&[("AMAZON", "Shopping")]));

        assert_eq!(fs::read_to_string(&path).unwrap(), MALFORMED);
    }
}
