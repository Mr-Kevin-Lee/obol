//! Checklist tracking (spec §13.1 Type D, D37) — the second slice of
//! recommendation tracking. A fixed, hardcoded list of 7 financial-plan
//! action items (not user-addable/removable this slice), each with a
//! tri-state status the user cycles through interactively. No
//! precondition/auto-hide mechanism yet (D37) — `NotApplicable` is the
//! manual escape hatch for an item that doesn't apply, and all 7 items
//! are always shown.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub struct ChecklistItem {
    pub id: &'static str,
    pub description: &'static str,
}

pub const CHECKLIST_ITEMS: &[ChecklistItem] = &[
    ChecklistItem {
        id: "estate_documents",
        description: "Estate documents (Will, Revocable Living Trust, Medical POA, Financial POA, Living Will, HIPAA release)",
    },
    ChecklistItem {
        id: "disability_insurance",
        description: "Own-occupation disability insurance in place",
    },
    ChecklistItem {
        id: "term_life_insurance",
        description: "Term life insurance in place",
    },
    ChecklistItem {
        id: "retirement_account_rollovers",
        description: "Retirement account rollover(s) completed",
    },
    ChecklistItem {
        id: "401k_roth_election",
        description: "Roth vs. pre-tax 401(k) election made",
    },
    ChecklistItem {
        id: "fsa_enrollment",
        description: "FSA enrollment",
    },
    ChecklistItem {
        id: "espp_participation",
        description: "ESPP participation / immediate-sale discipline",
    },
    ChecklistItem {
        id: "beneficiary_designations",
        description: "Beneficiary designations current (retirement accounts + life insurance)",
    },
    ChecklistItem {
        id: "trust_funding",
        description: "Trust funding confirmed (assets actually titled into the Revocable Living Trust, not just signed)",
    },
    ChecklistItem {
        id: "umbrella_insurance",
        description: "Umbrella liability coverage in place",
    },
    ChecklistItem {
        id: "poa_accessible",
        description: "Power of attorney / healthcare directives accessible (not just filed away)",
    },
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChecklistItemStatus {
    Complete,
    NotApplicable,
    /// D14-style forward-compat: an unrecognized future status string
    /// (e.g. read by an older binary after a later version adds a new
    /// variant) falls back here, never silently reading as Complete.
    #[serde(other)]
    Incomplete,
}

impl ChecklistItemStatus {
    pub fn label(&self) -> &'static str {
        match self {
            ChecklistItemStatus::Complete => "Complete",
            ChecklistItemStatus::Incomplete => "Incomplete",
            ChecklistItemStatus::NotApplicable => "N/A",
        }
    }

    /// `Incomplete -> Complete -> NotApplicable -> Incomplete`. Complete
    /// is one press away from a fresh item (the common case — marking
    /// something done); `NotApplicable` sits one press further, the
    /// less common path.
    pub fn cycle(self) -> Self {
        match self {
            ChecklistItemStatus::Incomplete => ChecklistItemStatus::Complete,
            ChecklistItemStatus::Complete => ChecklistItemStatus::NotApplicable,
            ChecklistItemStatus::NotApplicable => ChecklistItemStatus::Incomplete,
        }
    }
}

/// Keyed by `ChecklistItem::id`, not a fixed 7-field struct — decouples
/// persisted status data from the fixed-in-code item list, so a future
/// addition/removal from `CHECKLIST_ITEMS` never needs a schema
/// migration. `BTreeMap` (not `HashMap`) for deterministic, key-sorted
/// YAML output — stable, minimal diffs, same instinct as everywhere
/// else in this codebase that writes YAML by hand.
pub type ChecklistStatuses = BTreeMap<String, ChecklistItemStatus>;

/// Defaults to `Incomplete` for any item id not yet present in the map
/// — a fresh/empty checklist section in `rules.yaml` is fully valid,
/// no upfront population of all 7 keys required.
pub fn status_for(statuses: &ChecklistStatuses, item_id: &str) -> ChecklistItemStatus {
    statuses
        .get(item_id)
        .copied()
        .unwrap_or(ChecklistItemStatus::Incomplete)
}

/// `(complete_count, applicable_count)` — iterates `CHECKLIST_ITEMS`
/// (not the map's keys), so a stale/missing key is handled for free.
/// `NotApplicable` items are excluded from *both* the numerator and the
/// denominator: an item marked "doesn't apply to me" shouldn't count
/// against the ratio (that would make the number worse the more
/// honestly a user opts out of irrelevant items), matching the spec's
/// own "X/N complete" phrasing, where N already means "relevant items."
/// If every item is `NotApplicable`, returns `(0, 0)` — the caller
/// (dashboard rendering) must handle a `0/0` denominator without
/// dividing.
pub fn completion_summary(statuses: &ChecklistStatuses) -> (usize, usize) {
    let relevant: Vec<ChecklistItemStatus> = CHECKLIST_ITEMS
        .iter()
        .map(|item| status_for(statuses, item.id))
        .filter(|status| *status != ChecklistItemStatus::NotApplicable)
        .collect();
    let complete = relevant
        .iter()
        .filter(|status| **status == ChecklistItemStatus::Complete)
        .count();
    (complete, relevant.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checklist_items_const_has_exactly_eleven_items() {
        assert_eq!(CHECKLIST_ITEMS.len(), 11);
    }

    #[test]
    fn checklist_item_ids_are_unique() {
        let mut ids: Vec<&str> = CHECKLIST_ITEMS.iter().map(|item| item.id).collect();
        let original_len = ids.len();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), original_len);
    }

    #[test]
    fn cycle_moves_incomplete_to_complete() {
        assert_eq!(
            ChecklistItemStatus::Incomplete.cycle(),
            ChecklistItemStatus::Complete
        );
    }

    #[test]
    fn cycle_moves_complete_to_not_applicable() {
        assert_eq!(
            ChecklistItemStatus::Complete.cycle(),
            ChecklistItemStatus::NotApplicable
        );
    }

    #[test]
    fn cycle_moves_not_applicable_back_to_incomplete() {
        assert_eq!(
            ChecklistItemStatus::NotApplicable.cycle(),
            ChecklistItemStatus::Incomplete
        );
    }

    #[test]
    fn status_for_defaults_to_incomplete_for_a_missing_item_id() {
        let statuses = ChecklistStatuses::new();
        assert_eq!(
            status_for(&statuses, "estate_documents"),
            ChecklistItemStatus::Incomplete
        );
    }

    #[test]
    fn status_for_returns_the_stored_status_when_present() {
        let mut statuses = ChecklistStatuses::new();
        statuses.insert("estate_documents".to_string(), ChecklistItemStatus::Complete);
        assert_eq!(
            status_for(&statuses, "estate_documents"),
            ChecklistItemStatus::Complete
        );
    }

    #[test]
    fn completion_summary_counts_complete_items_in_the_numerator() {
        let mut statuses = ChecklistStatuses::new();
        statuses.insert("estate_documents".to_string(), ChecklistItemStatus::Complete);
        statuses.insert(
            "disability_insurance".to_string(),
            ChecklistItemStatus::Complete,
        );
        let (complete, _) = completion_summary(&statuses);
        assert_eq!(complete, 2);
    }

    #[test]
    fn completion_summary_excludes_not_applicable_items_from_the_denominator() {
        let mut statuses = ChecklistStatuses::new();
        statuses.insert(
            "espp_participation".to_string(),
            ChecklistItemStatus::NotApplicable,
        );
        let (_, applicable) = completion_summary(&statuses);
        // 11 items total, 1 marked N/A -> 10 applicable.
        assert_eq!(applicable, 10);
    }

    #[test]
    fn completion_summary_with_empty_statuses_reports_zero_of_eleven_complete() {
        let statuses = ChecklistStatuses::new();
        assert_eq!(completion_summary(&statuses), (0, 11));
    }

    #[test]
    fn completion_summary_all_not_applicable_reports_zero_of_zero() {
        let mut statuses = ChecklistStatuses::new();
        for item in CHECKLIST_ITEMS {
            statuses.insert(item.id.to_string(), ChecklistItemStatus::NotApplicable);
        }
        assert_eq!(completion_summary(&statuses), (0, 0));
    }

    #[test]
    fn label_returns_a_human_readable_string_for_each_status() {
        assert_eq!(ChecklistItemStatus::Complete.label(), "Complete");
        assert_eq!(ChecklistItemStatus::Incomplete.label(), "Incomplete");
        assert_eq!(ChecklistItemStatus::NotApplicable.label(), "N/A");
    }

    #[test]
    fn checklist_item_status_serializes_as_snake_case() {
        let json = serde_json::to_string(&ChecklistItemStatus::NotApplicable).unwrap();
        assert_eq!(json, "\"not_applicable\"");
    }

    #[test]
    fn an_unrecognized_status_string_deserializes_as_incomplete() {
        let status: ChecklistItemStatus = serde_json::from_str("\"some_future_variant\"").unwrap();
        assert_eq!(status, ChecklistItemStatus::Incomplete);
    }
}
