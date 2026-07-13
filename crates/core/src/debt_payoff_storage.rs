//! `rules.yaml`'s `debt_payoff` section (spec §13.1, D41) — a thin
//! wrapper over the shared load/save mechanics in `rules_storage.rs`,
//! mirroring `monthly_spend_storage.rs`'s exact shape.

use std::path::Path;

use crate::debt_payoff::DebtPayoffConfig;
use crate::rules_storage::{load_or_init_rules_file, save_rules_file, RulesStorageError};

/// Loads `rules.yaml`'s `debt_payoff` section, creating a default file
/// (7% threshold, no rates configured) if it doesn't exist yet.
pub fn load_or_init_debt_payoff_config(path: &Path) -> Result<DebtPayoffConfig, RulesStorageError> {
    Ok(load_or_init_rules_file(path)?.debt_payoff)
}

/// Saves debt-payoff config into `rules.yaml`. A read-modify-write,
/// same shape as every other section's save function — keeps sibling
/// sections from being reset to their defaults on save. Not called
/// from any UI flow yet (no in-TUI editing exists for this section,
/// same as every other threshold in this app) — exists for
/// completeness/testability and future use, matching the precedent
/// every other section's save function already set before its own UI
/// (if any) existed.
pub fn save_debt_payoff_config(
    path: &Path,
    config: &DebtPayoffConfig,
) -> Result<(), RulesStorageError> {
    let mut rules_file = load_or_init_rules_file(path)?;
    rules_file.debt_payoff = config.clone();
    save_rules_file(path, &rules_file)
}

/// Sets a single account's interest rate and persists it (spec D42) — a
/// read-modify-write over the whole `rules.yaml` file, same shape as
/// `checklist_storage::set_checklist_item_status`. Always overwrites
/// whatever's currently there for `source_id`, including a rate the
/// user hand-entered previously — the statement a parser just read is
/// treated as the source of truth, same stance already taken for
/// balance (no merge/precedence logic between manual and parsed
/// values).
pub fn save_debt_payoff_interest_rate(
    path: &Path,
    source_id: &str,
    rate: f64,
) -> Result<(), RulesStorageError> {
    let mut rules_file = load_or_init_rules_file(path)?;
    rules_file
        .debt_payoff
        .interest_rates
        .insert(source_id.to_string(), rate);
    save_rules_file(path, &rules_file)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checklist_storage::{load_or_init_checklist_statuses, set_checklist_item_status};
    use crate::debt_payoff::DebtInterestRates;
    use crate::emergency_fund_storage::{
        load_or_init_emergency_fund_thresholds, save_emergency_fund_thresholds,
    };
    use crate::{ChecklistItemStatus, ChecklistStatuses, EmergencyFundThresholds};
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "obol-debt-payoff-storage-test-{name}-{}.yaml",
            std::process::id()
        ))
    }

    fn configured_config() -> DebtPayoffConfig {
        let mut interest_rates = DebtInterestRates::new();
        interest_rates.insert("chase_credit_card".to_string(), 24.99);
        DebtPayoffConfig {
            high_interest_at_or_above: 7.0,
            interest_rates,
        }
    }

    #[test]
    fn load_or_init_creates_a_default_config_on_first_run() {
        let path = temp_path("first-run");
        let _ = fs::remove_file(&path);

        let config = load_or_init_debt_payoff_config(&path).unwrap();
        assert_eq!(config, DebtPayoffConfig::default());
        assert!(path.exists());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_creates_a_file_with_0600_permissions() {
        let path = temp_path("perms");
        let _ = fs::remove_file(&path);

        save_debt_payoff_config(&path, &configured_config()).unwrap();

        let perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_then_load_round_trips() {
        let path = temp_path("roundtrip");
        let _ = fs::remove_file(&path);

        save_debt_payoff_config(&path, &configured_config()).unwrap();
        let loaded = load_or_init_debt_payoff_config(&path).unwrap();

        assert_eq!(loaded, configured_config());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn saving_does_not_disturb_other_existing_sections() {
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

        save_debt_payoff_config(&path, &configured_config()).unwrap();

        let loaded_emergency_fund = load_or_init_emergency_fund_thresholds(&path).unwrap();
        assert_eq!(loaded_emergency_fund, emergency_fund_thresholds);

        let mut expected_checklist = ChecklistStatuses::new();
        expected_checklist.insert("estate_documents".to_string(), ChecklistItemStatus::Complete);
        let loaded_checklist = load_or_init_checklist_statuses(&path).unwrap();
        assert_eq!(loaded_checklist, expected_checklist);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_debt_payoff_interest_rate_persists_a_new_entry() {
        let path = temp_path("interest-rate-new");
        let _ = fs::remove_file(&path);

        save_debt_payoff_interest_rate(&path, "chase_sapphirereserve", 19.49).unwrap();
        let config = load_or_init_debt_payoff_config(&path).unwrap();

        assert_eq!(
            config.interest_rates.get("chase_sapphirereserve"),
            Some(&19.49)
        );

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_debt_payoff_interest_rate_overwrites_an_existing_entry() {
        // Spec D42: a freshly parsed statement's rate always overwrites
        // whatever's currently there, including a manually hand-entered
        // one — the statement is the source of truth.
        let path = temp_path("interest-rate-overwrite");
        let _ = fs::remove_file(&path);

        save_debt_payoff_interest_rate(&path, "chase_sapphirereserve", 99.99).unwrap();
        save_debt_payoff_interest_rate(&path, "chase_sapphirereserve", 19.49).unwrap();
        let config = load_or_init_debt_payoff_config(&path).unwrap();

        assert_eq!(
            config.interest_rates.get("chase_sapphirereserve"),
            Some(&19.49)
        );

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_debt_payoff_interest_rate_does_not_disturb_other_existing_sections() {
        let path = temp_path("interest-rate-preserve-siblings");
        let _ = fs::remove_file(&path);

        let emergency_fund_thresholds = EmergencyFundThresholds {
            target_monthly_expenses: 5000.0,
            red_below_months: 6.0,
            green_at_or_above_months: 9.0,
        };
        save_emergency_fund_thresholds(&path, &emergency_fund_thresholds).unwrap();
        set_checklist_item_status(&path, "estate_documents", ChecklistItemStatus::Complete)
            .unwrap();

        save_debt_payoff_interest_rate(&path, "chase_sapphirereserve", 19.49).unwrap();

        let loaded_emergency_fund = load_or_init_emergency_fund_thresholds(&path).unwrap();
        assert_eq!(loaded_emergency_fund, emergency_fund_thresholds);

        let mut expected_checklist = ChecklistStatuses::new();
        expected_checklist.insert("estate_documents".to_string(), ChecklistItemStatus::Complete);
        let loaded_checklist = load_or_init_checklist_statuses(&path).unwrap();
        assert_eq!(loaded_checklist, expected_checklist);

        fs::remove_file(&path).ok();
    }
}
