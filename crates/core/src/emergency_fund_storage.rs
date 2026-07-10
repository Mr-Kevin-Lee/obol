//! `rules.yaml` storage for emergency fund thresholds (spec §13, D36).
//! Generically named/shaped so a future rule type (e.g. a v0.5b
//! checklist) can add its own section to the same file later without
//! restructuring it — see [`RulesFile`]. The public API here stays
//! narrow and concrete (just emergency-fund thresholds), mirroring
//! `sources.rs`'s YAML/atomic-write pattern exactly.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::emergency_fund::EmergencyFundThresholds;

/// The on-disk shape of `rules.yaml` — a small, growing set of named
/// sections, one per rule type. Only `emergency_fund` exists this
/// slice; a future section is added the same way `SourceConfig`
/// additions are, a new field here with `#[serde(default)]` so an
/// existing file without it still parses. Private: callers only ever
/// see the unwrapped `EmergencyFundThresholds` they asked for, the same
/// precedent `sources.rs`'s private `SourcesFile` already sets.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct RulesFile {
    #[serde(default)]
    emergency_fund: EmergencyFundThresholds,
}

#[derive(Debug, Error)]
pub enum EmergencyFundThresholdsStorageError {
    #[error("rules.yaml could not be parsed: {0}")]
    Parse(serde_saphyr::Error),
    #[error("failed to write rules.yaml: {0}")]
    Serialize(serde_saphyr::ser_error::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
}

/// Loads `rules.yaml`'s `emergency_fund` section, creating a default
/// file (target unconfigured, spec §13.1's illustrative red/green
/// bands) if it doesn't exist yet.
pub fn load_or_init_emergency_fund_thresholds(
    path: &Path,
) -> Result<EmergencyFundThresholds, EmergencyFundThresholdsStorageError> {
    Ok(load_or_init_rules_file(path)?.emergency_fund)
}

/// Saves emergency-fund thresholds into `rules.yaml`. A read-modify-
/// write (loads the existing file, replaces just its `emergency_fund`
/// field, writes the whole file back) rather than a blind overwrite of
/// a freshly-constructed `RulesFile` — this matters once a second
/// section is ever added to [`RulesFile`] as a new field, so that
/// write picks up whatever this run already loaded for it rather than
/// silently resetting it to a default. **Not yet a guarantee against a
/// key `RulesFile` doesn't know about at all** — this struct has no
/// catch-all field, so an *unrecognized* top-level key in an
/// externally-hand-edited file would still be dropped on the next save,
/// same as any other typed-struct round-trip in this codebase (e.g.
/// `sources.rs`'s `SourcesFile`). Worth revisiting if that ever matters
/// in practice, not solved speculatively here.
pub fn save_emergency_fund_thresholds(
    path: &Path,
    thresholds: &EmergencyFundThresholds,
) -> Result<(), EmergencyFundThresholdsStorageError> {
    let mut rules_file = load_or_init_rules_file(path)?;
    rules_file.emergency_fund = *thresholds;
    write_atomically(path, &rules_file)
}

fn load_or_init_rules_file(
    path: &Path,
) -> Result<RulesFile, EmergencyFundThresholdsStorageError> {
    if !path.exists() {
        let default = RulesFile::default();
        write_atomically(path, &default)?;
        return Ok(default);
    }

    let contents = fs::read_to_string(path)?;
    serde_saphyr::from_str(&contents).map_err(EmergencyFundThresholdsStorageError::Parse)
}

/// Atomic write (temp file + rename) with `0600` permissions (§4),
/// mirroring `sources.rs::write_atomically` — deliberately not shared
/// with it, matching the established "each storage module owns its own
/// near-identical block" precedent.
fn write_atomically(
    path: &Path,
    rules_file: &RulesFile,
) -> Result<(), EmergencyFundThresholdsStorageError> {
    let yaml = serde_saphyr::to_string(rules_file)
        .map_err(EmergencyFundThresholdsStorageError::Serialize)?;

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

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "obol-rules-test-{name}-{}.yaml",
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
        assert!(matches!(
            err,
            EmergencyFundThresholdsStorageError::Parse(_)
        ));
        assert!(err.to_string().contains("could not be parsed"));

        fs::remove_file(&path).ok();
    }
}
