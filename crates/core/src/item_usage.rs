//! Plaid Item usage counter (spec §7.1, decision D8). Plaid has no API
//! to query remaining Item quota, so this lifetime counter is
//! maintained by the app itself. Scoped here to the counter's data
//! model and threshold logic — where it's persisted (alongside
//! `sources.yaml`, same `0600` protection per §4) is a storage-layer
//! concern for a later task, not this one.

use serde::{Deserialize, Serialize};

/// Plaid's Trial plan Item cap (§7) — a hard block at this count.
pub const PLAID_ITEM_LIMIT: u32 = 10;

/// Warn clearly at this count, before the hard block (§7.1).
pub const PLAID_ITEM_WARNING_THRESHOLD: u32 = 8;

/// Lifetime count of Plaid Items ever created by this app. Deliberately
/// has no way to decrement — `/item/remove` doesn't free the Trial cap
/// (§7), so removing a source must never lower this number.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ItemUsageCounter {
    count: u32,
}

impl ItemUsageCounter {
    pub fn new() -> Self {
        Self { count: 0 }
    }

    pub fn count(&self) -> u32 {
        self.count
    }

    /// Called exactly once, at the moment a new Plaid Item is
    /// successfully created — the public_token/access_token exchange
    /// succeeding during the Sources screen's "Connect via Plaid" flow
    /// (§10.1). Never called speculatively, and never undone.
    pub fn increment(&mut self) {
        self.count += 1;
    }

    /// Whether to show the "approaching the limit" warning (§7.1).
    pub fn is_at_warning_threshold(&self) -> bool {
        self.count >= PLAID_ITEM_WARNING_THRESHOLD
    }

    /// Whether a new "Connect via Plaid" flow should be blocked
    /// entirely (§7.1 — a hard block at 10/10, not just a warning).
    pub fn is_blocked(&self) -> bool {
        self.count >= PLAID_ITEM_LIMIT
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_counter_starts_at_zero() {
        let counter = ItemUsageCounter::new();
        assert_eq!(counter.count(), 0);
        assert!(!counter.is_at_warning_threshold());
        assert!(!counter.is_blocked());
    }

    #[test]
    fn increment_increases_count_by_one() {
        let mut counter = ItemUsageCounter::new();
        counter.increment();
        assert_eq!(counter.count(), 1);
        counter.increment();
        assert_eq!(counter.count(), 2);
    }

    #[test]
    fn removing_a_source_never_decrements_the_counter() {
        // There's no decrement() method at all — this documents the
        // intended behavior directly: incrementing several times, then
        // simulating "a source was removed" (which, by design, means
        // doing nothing to this counter), leaves the count unchanged.
        let mut counter = ItemUsageCounter::new();
        counter.increment();
        counter.increment();
        counter.increment();
        let count_before_removal = counter.count();

        // "Removal" — no call happens here, on purpose.

        assert_eq!(counter.count(), count_before_removal);
        assert_eq!(counter.count(), 3);
    }

    #[test]
    fn not_at_warning_threshold_below_eight() {
        let mut counter = ItemUsageCounter::new();
        for _ in 0..7 {
            counter.increment();
        }
        assert_eq!(counter.count(), 7);
        assert!(!counter.is_at_warning_threshold());
    }

    #[test]
    fn at_warning_threshold_at_eight_and_above() {
        let mut counter = ItemUsageCounter::new();
        for _ in 0..8 {
            counter.increment();
        }
        assert!(counter.is_at_warning_threshold());

        counter.increment();
        assert!(counter.is_at_warning_threshold());
    }

    #[test]
    fn not_blocked_below_ten() {
        let mut counter = ItemUsageCounter::new();
        for _ in 0..9 {
            counter.increment();
        }
        assert_eq!(counter.count(), 9);
        assert!(!counter.is_blocked());
    }

    #[test]
    fn blocked_at_ten_and_above() {
        let mut counter = ItemUsageCounter::new();
        for _ in 0..10 {
            counter.increment();
        }
        assert!(counter.is_blocked());
    }

    #[test]
    fn serializes_and_deserializes_correctly() {
        let mut counter = ItemUsageCounter::new();
        counter.increment();
        counter.increment();
        counter.increment();

        let json = serde_json::to_string(&counter).unwrap();
        let round_tripped: ItemUsageCounter = serde_json::from_str(&json).unwrap();

        assert_eq!(round_tripped, counter);
        assert_eq!(round_tripped.count(), 3);
    }
}
