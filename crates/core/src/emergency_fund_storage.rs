//! `rules.yaml`'s `emergency_fund` section (spec §13, D36) — a thin
//! wrapper over the shared load/save mechanics in `rules_storage.rs`
//! (extracted in D37 once `checklist` became a second real section of
//! the same file). The public API here stays narrow and concrete (just
//! emergency-fund thresholds), no generic `Recommendation` wrapper.

use std::path::Path;

use crate::emergency_fund::EmergencyFundThresholds;
use crate::rules_storage::{load_or_init_rules_file, save_rules_file, RulesStorageError};

/// Loads `rules.yaml`'s `emergency_fund` section, creating a default
/// file (target unconfigured, spec §13.1's illustrative red/green
/// bands) if it doesn't exist yet.
pub fn load_or_init_emergency_fund_thresholds(
    path: &Path,
) -> Result<EmergencyFundThresholds, RulesStorageError> {
    Ok(load_or_init_rules_file(path)?.emergency_fund)
}

/// Saves emergency-fund thresholds into `rules.yaml`. A read-modify-
/// write (loads the existing file, replaces just its `emergency_fund`
/// field, writes the whole file back) rather than a blind overwrite —
/// this is what keeps a sibling section (e.g. `checklist`) already
/// present in the file from being reset to its default on save.
pub fn save_emergency_fund_thresholds(
    path: &Path,
    thresholds: &EmergencyFundThresholds,
) -> Result<(), RulesStorageError> {
    let mut rules_file = load_or_init_rules_file(path)?;
    rules_file.emergency_fund = *thresholds;
    save_rules_file(path, &rules_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checklist::{ChecklistItemStatus, ChecklistStatuses};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "obol-emergency-fund-storage-test-{name}-{}.yaml",
            std::process::id()
        ))
    }

    fn configured_thresholds() -> EmergencyFundThresholds {
        EmergencyFundThresholds {
            target_monthly_expenses: 5000.0,
            red_below_months: 6.0,
            green_at_or_above_months: 9.0,
        }
    }

    #[test]
    fn load_or_init_creates_a_default_rules_file_on_first_run() {
        let path = temp_path("first-run");
        let _ = fs::remove_file(&path);

        let thresholds = load_or_init_emergency_fund_thresholds(&path).unwrap();
        assert_eq!(thresholds, EmergencyFundThresholds::default());
        assert!(path.exists());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_creates_a_file_with_0600_permissions() {
        let path = temp_path("perms");
        let _ = fs::remove_file(&path);

        save_emergency_fund_thresholds(&path, &configured_thresholds()).unwrap();

        let perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_path("roundtrip");
        let _ = fs::remove_file(&path);

        save_emergency_fund_thresholds(&path, &configured_thresholds()).unwrap();
        let loaded = load_or_init_emergency_fund_thresholds(&path).unwrap();

        assert_eq!(loaded, configured_thresholds());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn saving_over_an_existing_file_updates_its_emergency_fund_section() {
        let path = temp_path("update-existing");
        let _ = fs::remove_file(&path);

        fs::write(
            &path,
            "emergency_fund:\n  target_monthly_expenses: 1000.0\n  red_below_months: 6.0\n  green_at_or_above_months: 9.0\n",
        )
        .unwrap();

        save_emergency_fund_thresholds(&path, &configured_thresholds()).unwrap();
        let loaded = load_or_init_emergency_fund_thresholds(&path).unwrap();

        assert_eq!(loaded, configured_thresholds());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn a_malformed_file_produces_a_clear_parse_error() {
        let path = temp_path("malformed");
        fs::write(&path, "emergency_fund: [this is not valid: yaml: at all: -").unwrap();

        let err = load_or_init_emergency_fund_thresholds(&path).unwrap_err();
        assert!(matches!(err, RulesStorageError::Parse(_)));
        assert!(err.to_string().contains("could not be parsed"));

        fs::remove_file(&path).ok();
    }

    #[test]
    fn saving_emergency_fund_thresholds_does_not_disturb_an_existing_checklist_section() {
        let path = temp_path("preserve-checklist");
        let _ = fs::remove_file(&path);

        // Establish a checklist section first, via the sibling module's
        // own writer, then save emergency-fund thresholds and confirm
        // the checklist entry is still there afterward — the whole
        // point of the rules_storage.rs extraction (D37).
        crate::checklist_storage::set_checklist_item_status(
            &path,
            "estate_documents",
            ChecklistItemStatus::Complete,
        )
        .unwrap();

        save_emergency_fund_thresholds(&path, &configured_thresholds()).unwrap();

        let mut expected = ChecklistStatuses::new();
        expected.insert("estate_documents".to_string(), ChecklistItemStatus::Complete);
        let checklist = crate::checklist_storage::load_or_init_checklist_statuses(&path).unwrap();
        assert_eq!(checklist, expected);

        fs::remove_file(&path).ok();
    }
}
