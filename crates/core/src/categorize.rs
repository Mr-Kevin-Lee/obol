//! Category resolution (spec §11.4, D44). Computed whenever transactions
//! are read, never stored on them, so a rule edit re-categorizes the past
//! as well as the future. No LLM or network call, ever (#35).
//!
//! **Contract (task 111b), implemented by task 114.** First match wins:
//!
//! 1. A rule in `merchant_rules.yaml`, matched on the exact merchant key.
//! 2. Transfer pairing: an equal and opposite amount (within a cent) on a
//!    *different* source, posted within 5 days; each transaction pairs at
//!    most once. Both sides become [`TRANSFER`].
//! 3. Built-in defaults: common merchants plus transfer keywords
//!    (`AUTOPAY`, `PAYMENT THANK YOU`…). Matched by **word prefix**: the
//!    longest table entry whose words are a prefix of the key's words
//!    (`UBER` matches `UBER` and `UBER TRIP`, but not `UBERRIMA`; `UBER
//!    EATS` beats `UBER` for the key `UBER EATS`).
//!    Built as a table so a large generated one — e.g. from OpenStreetMap's
//!    Name Suggestion Index, a planned follow-up — can replace the
//!    hand-picked one without a code change.
//! 4. [`UNCATEGORIZED`].

use serde::Serialize;

use crate::merchant_rules_storage::MerchantRules;
use crate::transaction::Transaction;

pub const UNCATEGORIZED: &str = "Uncategorized";
pub const TRANSFER: &str = "Transfer";
pub const INCOME: &str = "Income";

/// A transaction with its resolved merchant key and category.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CategorizedTransaction {
    pub transaction: Transaction,
    pub merchant_key: String,
    pub category: String,
}

impl CategorizedTransaction {
    pub fn is_transfer(&self) -> bool {
        self.category == TRANSFER
    }
}

/// Every category the app offers, in display order: Simplifi's top-level
/// list (including [`INCOME`], [`TRANSFER`] and [`UNCATEGORIZED`]), then
/// any category that appears in `rules` but not in that list, sorted.
#[allow(unused_variables)] // stub until task 114
pub fn available_categories(rules: &MerchantRules) -> Vec<String> {
    todo!("task 114")
}

/// Resolves every transaction, returned in input order. Transfer pairing
/// looks across all of `transactions`, so callers pass adjacent months too
/// (a payment on the 31st pairs with one on the 2nd) and filter afterwards.
#[allow(unused_variables)] // stub until task 114
pub fn categorize(
    transactions: &[Transaction],
    rules: &MerchantRules,
) -> Vec<CategorizedTransaction> {
    todo!("task 114")
}
