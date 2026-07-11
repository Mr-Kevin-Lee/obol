//! `rules.yaml`'s `checklist` section (spec §13.1 Type D, D37) — a thin
//! wrapper over the shared load/save mechanics in `rules_storage.rs`,
//! mirroring `emergency_fund_storage.rs`'s exact shape.

use std::path::Path;

use crate::checklist::{ChecklistItemStatus, ChecklistStatuses};
use crate::rules_storage::{load_or_init_rules_file, save_rules_file, RulesStorageError};

/// Loads `rules.yaml`'s `checklist` section, creating a default file
/// (an empty map — every item defaults to `Incomplete` via
/// `checklist::status_for`) if it doesn't exist yet.
pub fn load_or_init_checklist_statuses(path: &Path) -> Result<ChecklistStatuses, RulesStorageError> {
    Ok(load_or_init_rules_file(path)?.checklist)
}

/// Sets a single checklist item's status and persists it — a read-
/// modify-write over the whole `rules.yaml` file (loads it, updates
/// just this one map entry, writes it all back), same shape as
/// `save_emergency_fund_thresholds`. Doesn't validate `item_id` against
/// `checklist::CHECKLIST_ITEMS` — the only caller
/// (`recommendations_screen.rs`) always passes an id sourced directly
/// from that const list, and a stale/unrecognized key left in a
/// hand-edited file is already handled by `status_for`/
/// `completion_summary` iterating the const list, not the map.
pub fn set_checklist_item_status(
    path: &Path,
    item_id: &str,
    status: ChecklistItemStatus,
) -> Result<(), RulesStorageError> {
    let mut rules_file = load_or_init_rules_file(path)?;
    rules_file.checklist.insert(item_id.to_string(), status);
    save_rules_file(path, &rules_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::emergency_fund_storage::save_emergency_fund_thresholds;
    use crate::EmergencyFundThresholds;
    use std::fs;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "obol-checklist-storage-test-{name}-{}.yaml",
            std::process::id()
        ))
    }

    #[test]
    fn load_or_init_returns_an_empty_map_on_a_fresh_rules_file() {
        let path = temp_path("first-run");
        let _ = fs::remove_file(&path);

        let statuses = load_or_init_checklist_statuses(&path).unwrap();
        assert!(statuses.is_empty());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn set_checklist_item_status_persists_a_new_entry() {
        let path = temp_path("persist-new");
        let _ = fs::remove_file(&path);

        set_checklist_item_status(&path, "estate_documents", ChecklistItemStatus::Complete)
            .unwrap();
        let statuses = load_or_init_checklist_statuses(&path).unwrap();

        assert_eq!(
            statuses.get("estate_documents"),
            Some(&ChecklistItemStatus::Complete)
        );

        fs::remove_file(&path).ok();
    }

    #[test]
    fn set_checklist_item_status_updates_an_existing_entry() {
        let path = temp_path("update-existing");
        let _ = fs::remove_file(&path);

        set_checklist_item_status(&path, "estate_documents", ChecklistItemStatus::Complete)
            .unwrap();
        set_checklist_item_status(&path, "estate_documents", ChecklistItemStatus::NotApplicable)
            .unwrap();
        let statuses = load_or_init_checklist_statuses(&path).unwrap();

        assert_eq!(
            statuses.get("estate_documents"),
            Some(&ChecklistItemStatus::NotApplicable)
        );

        fs::remove_file(&path).ok();
    }

    #[test]
    fn set_checklist_item_status_does_not_disturb_the_emergency_fund_section() {
        let path = temp_path("preserve-emergency-fund");
        let _ = fs::remove_file(&path);

        let thresholds = EmergencyFundThresholds {
            target_monthly_expenses: 5000.0,
            red_below_months: 6.0,
            green_at_or_above_months: 9.0,
        };
        save_emergency_fund_thresholds(&path, &thresholds).unwrap();

        set_checklist_item_status(&path, "estate_documents", ChecklistItemStatus::Complete)
            .unwrap();

        let loaded =
            crate::emergency_fund_storage::load_or_init_emergency_fund_thresholds(&path).unwrap();
        assert_eq!(loaded, thresholds);

        fs::remove_file(&path).ok();
    }
}
