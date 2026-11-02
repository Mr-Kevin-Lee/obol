//! `plan.yaml` (spec §13.6, D44): category targets, confirmed series and
//! dismissed suggestions.
//!
//! **Contract (task 111b), implemented by task 115.** A separate file from
//! `rules.yaml` for the same reason as `merchant_rules.yaml`. Writes go
//! through `atomic_write`.

use std::path::Path;

use thiserror::Error;

use crate::atomic_write::write_atomically;
use crate::spending_plan::PlanConfig;

#[derive(Debug, Error)]
pub enum PlanStorageError {
    #[error("plan.yaml could not be parsed: {0}")]
    Parse(serde_saphyr::Error),
    #[error("failed to write plan.yaml: {0}")]
    Serialize(serde_saphyr::ser_error::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Loads `plan.yaml`, creating an empty one if it doesn't exist.
pub fn load_or_init_plan(path: &Path) -> Result<PlanConfig, PlanStorageError> {
    if !path.exists() {
        let plan = PlanConfig::default();
        save_plan(path, &plan)?;
        return Ok(plan);
    }
    let contents = std::fs::read_to_string(path)?;
    serde_saphyr::from_str(&contents).map_err(PlanStorageError::Parse)
}

pub fn save_plan(path: &Path, plan: &PlanConfig) -> Result<(), PlanStorageError> {
    let yaml = serde_saphyr::to_string(plan).map_err(PlanStorageError::Serialize)?;
    write_atomically(path, yaml.as_bytes())?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::path::PathBuf;

    use super::*;
    use crate::spending_plan::{RecurringSeries, SeriesKind};

    fn temp_path(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "obol-plan-storage-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("plan.yaml")
    }

    fn populated_plan() -> PlanConfig {
        PlanConfig {
            targets: [("Groceries".to_string(), 600.5)].into(),
            series: vec![
                RecurringSeries {
                    merchant_key: "NETFLIX".into(),
                    kind: SeriesKind::Bill,
                    expected_amount: 15.49,
                    due_day: 12,
                },
                RecurringSeries {
                    merchant_key: "ACME PAYROLL".into(),
                    kind: SeriesKind::Income,
                    expected_amount: 4430.81,
                    due_day: 1,
                },
            ],
            dismissed: ["AMAZON".to_string()].into(),
        }
    }

    #[test]
    fn should_return_an_empty_plan_when_the_file_is_missing() {
        let path = temp_path("init-empty");

        let plan = load_or_init_plan(&path).unwrap();

        assert_eq!(plan, PlanConfig::default());
    }

    #[test]
    fn should_create_the_file_when_missing() {
        let path = temp_path("init-creates");

        load_or_init_plan(&path).unwrap();

        assert!(path.exists());
    }

    #[test]
    fn should_reload_an_initialized_file_as_an_empty_plan() {
        let path = temp_path("init-reload");
        load_or_init_plan(&path).unwrap();

        let plan = load_or_init_plan(&path).unwrap();

        assert_eq!(plan, PlanConfig::default());
    }

    #[test]
    fn should_round_trip_targets_series_and_dismissed() {
        let path = temp_path("round-trip");
        save_plan(&path, &populated_plan()).unwrap();

        let loaded = load_or_init_plan(&path).unwrap();

        assert_eq!(loaded, populated_plan());
    }

    #[test]
    fn should_write_the_file_with_0600_permissions() {
        let path = temp_path("perms");

        save_plan(&path, &populated_plan()).unwrap();

        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }

    #[test]
    fn should_return_a_parse_error_for_malformed_yaml() {
        let path = temp_path("malformed");
        fs::write(&path, "targets: [this is not: valid: yaml: -").unwrap();

        let err = load_or_init_plan(&path).unwrap_err();

        assert!(matches!(err, PlanStorageError::Parse(_)));
    }
}
