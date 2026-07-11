//! `rules.yaml`'s `monthly_spend` section (spec §13.4, D39) — a thin
//! wrapper over the shared load/save mechanics in `rules_storage.rs`,
//! mirroring `emergency_fund_storage.rs`'s exact shape.

use std::path::Path;

use crate::monthly_spend::MonthlySpendThresholds;
use crate::rules_storage::{load_or_init_rules_file, save_rules_file, RulesStorageError};

/// Loads `rules.yaml`'s `monthly_spend` section, creating a default
/// file (the user-given $8,000/$11,000 starting bands) if it doesn't
/// exist yet.
pub fn load_or_init_monthly_spend_thresholds(
    path: &Path,
) -> Result<MonthlySpendThresholds, RulesStorageError> {
    Ok(load_or_init_rules_file(path)?.monthly_spend)
}

/// Saves monthly-spend thresholds into `rules.yaml`. A read-modify-
/// write (loads the existing file, replaces just its `monthly_spend`
/// field, writes the whole file back), same shape as
/// `save_emergency_fund_thresholds` — keeps the sibling
/// `emergency_fund`/`checklist` sections from being reset to their
/// defaults on save.
pub fn save_monthly_spend_thresholds(
    path: &Path,
    thresholds: &MonthlySpendThresholds,
) -> Result<(), RulesStorageError> {
    let mut rules_file = load_or_init_rules_file(path)?;
    rules_file.monthly_spend = *thresholds;
    save_rules_file(path, &rules_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checklist_storage::{load_or_init_checklist_statuses, set_checklist_item_status};
    use crate::emergency_fund_storage::{
        load_or_init_emergency_fund_thresholds, save_emergency_fund_thresholds,
    };
    use crate::{ChecklistItemStatus, ChecklistStatuses, EmergencyFundThresholds};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "obol-monthly-spend-storage-test-{name}-{}.yaml",
            std::process::id()
        ))
    }

    fn configured_thresholds() -> MonthlySpendThresholds {
        MonthlySpendThresholds {
            yellow_at_or_above: 8000.0,
            red_at_or_above: 11000.0,
        }
    }

    #[test]
    fn load_or_init_creates_a_default_rules_file_with_user_given_thresholds_on_first_run() {
        let path = temp_path("first-run");
        let _ = fs::remove_file(&path);

        let thresholds = load_or_init_monthly_spend_thresholds(&path).unwrap();
        assert_eq!(thresholds, MonthlySpendThresholds::default());
        assert!(path.exists());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_creates_a_file_with_0600_permissions() {
        let path = temp_path("perms");
        let _ = fs::remove_file(&path);

        save_monthly_spend_thresholds(&path, &configured_thresholds()).unwrap();

        let perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_path("roundtrip");
        let _ = fs::remove_file(&path);

        save_monthly_spend_thresholds(&path, &configured_thresholds()).unwrap();
        let loaded = load_or_init_monthly_spend_thresholds(&path).unwrap();

        assert_eq!(loaded, configured_thresholds());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn saving_over_an_existing_file_updates_its_monthly_spend_section() {
        let path = temp_path("update-existing");
        let _ = fs::remove_file(&path);

        fs::write(
            &path,
            "monthly_spend:\n  yellow_at_or_above: 1000.0\n  red_at_or_above: 2000.0\n",
        )
        .unwrap();

        save_monthly_spend_thresholds(&path, &configured_thresholds()).unwrap();
        let loaded = load_or_init_monthly_spend_thresholds(&path).unwrap();

        assert_eq!(loaded, configured_thresholds());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn a_malformed_file_produces_a_clear_parse_error() {
        let path = temp_path("malformed");
        fs::write(&path, "monthly_spend: [this is not valid: yaml: at all: -").unwrap();

        let err = load_or_init_monthly_spend_thresholds(&path).unwrap_err();
        assert!(matches!(err, RulesStorageError::Parse(_)));
        assert!(err.to_string().contains("could not be parsed"));

        fs::remove_file(&path).ok();
    }

    #[test]
    fn saving_monthly_spend_thresholds_does_not_disturb_existing_emergency_fund_and_checklist_sections(
    ) {
        let path = temp_path("preserve-siblings");
        let _ = fs::remove_file(&path);

        let emergency_fund_thresholds = EmergencyFundThresholds {
            target_monthly_expenses: 5000.0,
            red_below_months: 6.0,
            green_at_or_above_months: 9.0,
        };
        save_emergency_fund_thresholds(&path, &emergency_fund_thresholds).unwrap();
        set_checklist_item_status(&path, "estate_documents", ChecklistItemStatus::Complete)
            .unwrap();

        save_monthly_spend_thresholds(&path, &configured_thresholds()).unwrap();

        let loaded_emergency_fund = load_or_init_emergency_fund_thresholds(&path).unwrap();
        assert_eq!(loaded_emergency_fund, emergency_fund_thresholds);

        let mut expected_checklist = ChecklistStatuses::new();
        expected_checklist.insert("estate_documents".to_string(), ChecklistItemStatus::Complete);
        let loaded_checklist = load_or_init_checklist_statuses(&path).unwrap();
        assert_eq!(loaded_checklist, expected_checklist);

        fs::remove_file(&path).ok();
    }
}
