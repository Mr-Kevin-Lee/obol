//! `merchant_rules.yaml` (spec §11.4, D44): merchant key → category, the
//! first and strongest layer of categorization. Written by an edit in the
//! app and seeded by the one-time Simplifi import.
//!
//! **Contract (task 111b), implemented by task 114.** A separate file from
//! `rules.yaml` on purpose: adding a section there would mean editing
//! `rules_storage.rs`, which #37 changes. Writes go through `atomic_write`.

use std::collections::BTreeMap;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

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
#[allow(unused_variables)] // stub until task 114
pub fn load_or_init_merchant_rules(path: &Path) -> Result<MerchantRules, MerchantRulesError> {
    todo!("task 114")
}

/// Sets one merchant's category (read, modify, write) and returns the
/// updated rules. Overwrites any existing rule for that merchant.
#[allow(unused_variables)] // stub until task 114
pub fn set_merchant_category(
    path: &Path,
    merchant_key: &str,
    category: &str,
) -> Result<MerchantRules, MerchantRulesError> {
    todo!("task 114")
}
