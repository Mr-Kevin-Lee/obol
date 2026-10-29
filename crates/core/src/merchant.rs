//! Merchant keys (spec §11.4, D44): one canonical name per merchant, so
//! every way a bank writes "Amazon" lands on the same category rule.
//!
//! **Contract (task 111b), implemented by task 113.** `merchant_key`:
//! uppercases, strips payment-processor prefixes (`SQ *`, `TST*`,
//! `PAYPAL *`, `POS `…), store numbers, card last-4 digits, phone numbers,
//! URLs and trailing city/state/zip, collapses whitespace, then applies a
//! small alias table (`AMZN` → `AMAZON`). Two properties other code relies
//! on, which task 113 must keep:
//!
//! 1. **Idempotent**: `merchant_key(&merchant_key(x)) == merchant_key(x)`.
//! 2. **A bare uppercase merchant name is already a key**: `merchant_key("AMAZON")
//!    == "AMAZON"`, `merchant_key("UBER EATS") == "UBER EATS"`. The
//!    categorize and plan streams test against canonical names like these
//!    before this module's real implementation lands.

/// **Stub until task 113** — correct only for inputs that are already
/// canonical, which is exactly what the other streams' tests use.
pub fn merchant_key(description: &str) -> String {
    description.trim().to_uppercase()
}

/// A merchant key as people would write it: `UBER EATS` → `Uber Eats`.
/// **Stub until task 113.**
pub fn display_name(merchant_key: &str) -> String {
    merchant_key.to_string()
}
