//! Shared `rules.yaml` load/save mechanics (spec D36/D37) — extracted
//! once a second real rule-type section (`checklist`, D37) needed to
//! share the same physical file `emergency_fund` (D36) already used.
//! Sharing this avoids two independent read-modify-write cycles racing
//! on one file, or one silently dropping the other's field on save —
//! a real correctness hazard, not just duplication. `RulesFile` and the
//! load/save functions here stay `pub(crate)`: only each rule type's
//! own thin wrapper module (`emergency_fund_storage.rs`,
//! `checklist_storage.rs`) is ever exported from the crate.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::checklist::ChecklistStatuses;
use crate::emergency_fund::EmergencyFundThresholds;

/// The on-disk shape of `rules.yaml` — a small, growing set of named
/// sections, one per rule type. A future section is added the same way
/// `SourceConfig` additions are: a new field here with
/// `#[serde(default)]` so an existing file without it still parses.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub(crate) struct RulesFile {
    #[serde(default)]
    pub(crate) emergency_fund: EmergencyFundThresholds,
    #[serde(default)]
    pub(crate) checklist: ChecklistStatuses,
}

#[derive(Debug, Error)]
pub enum RulesStorageError {
    #[error("rules.yaml could not be parsed: {0}")]
    Parse(serde_saphyr::Error),
    #[error("failed to write rules.yaml: {0}")]
    Serialize(serde_saphyr::ser_error::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Loads `rules.yaml`, creating a default file (both sections at their
/// defaults) if it doesn't exist yet.
pub(crate) fn load_or_init_rules_file(path: &Path) -> Result<RulesFile, RulesStorageError> {
    if !path.exists() {
        let default = RulesFile::default();
        save_rules_file(path, &default)?;
        return Ok(default);
    }

    let contents = fs::read_to_string(path)?;
    serde_saphyr::from_str(&contents).map_err(RulesStorageError::Parse)
}

/// Atomic write (temp file + rename) with `0600` permissions (§4),
/// mirroring `sources.rs::write_atomically` — deliberately not shared
/// with it, matching the established "each storage module owns its own
/// near-identical block" precedent.
pub(crate) fn save_rules_file(path: &Path, rules_file: &RulesFile) -> Result<(), RulesStorageError> {
    let yaml = serde_saphyr::to_string(rules_file).map_err(RulesStorageError::Serialize)?;

    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }

    let temp_path = path.with_extension("yaml.tmp");
    {
        let mut file = fs::File::create(&temp_path)?;
        file.write_all(yaml.as_bytes())?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
    }
    fs::rename(&temp_path, path)?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::checklist::ChecklistItemStatus;

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "obol-rules-storage-test-{name}-{}.yaml",
            std::process::id()
        ))
    }

    #[test]
    fn load_or_init_creates_a_default_rules_file_with_both_sections_empty_defaults() {
        let path = temp_path("first-run");
        let _ = fs::remove_file(&path);

        let rules_file = load_or_init_rules_file(&path).unwrap();
        assert_eq!(rules_file.emergency_fund, EmergencyFundThresholds::default());
        assert!(rules_file.checklist.is_empty());
        assert!(path.exists());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn rules_file_round_trips_with_both_sections_populated() {
        let path = temp_path("roundtrip-both");
        let _ = fs::remove_file(&path);

        let mut rules_file = RulesFile {
            emergency_fund: EmergencyFundThresholds {
                target_monthly_expenses: 5000.0,
                red_below_months: 6.0,
                green_at_or_above_months: 9.0,
            },
            checklist: ChecklistStatuses::new(),
        };
        rules_file
            .checklist
            .insert("estate_documents".to_string(), ChecklistItemStatus::Complete);

        save_rules_file(&path, &rules_file).unwrap();
        let loaded = load_or_init_rules_file(&path).unwrap();

        assert_eq!(loaded.emergency_fund, rules_file.emergency_fund);
        assert_eq!(loaded.checklist, rules_file.checklist);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn a_malformed_rules_file_produces_a_clear_parse_error() {
        let path = temp_path("malformed");
        fs::write(&path, "emergency_fund: [this is not valid: yaml: at all: -").unwrap();

        let err = load_or_init_rules_file(&path).unwrap_err();
        assert!(matches!(err, RulesStorageError::Parse(_)));
        assert!(err.to_string().contains("could not be parsed"));

        fs::remove_file(&path).ok();
    }

    #[test]
    fn save_creates_a_file_with_0600_permissions() {
        let path = temp_path("perms");
        let _ = fs::remove_file(&path);

        save_rules_file(&path, &RulesFile::default()).unwrap();

        let perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);

        fs::remove_file(&path).ok();
    }
}
