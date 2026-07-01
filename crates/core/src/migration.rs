use thiserror::Error;

use crate::snapshot::Snapshot;

/// The schema version this build writes and fully understands (spec §11.3).
pub const CURRENT_SCHEMA_VERSION: u32 = 1;

/// The oldest schema version this build can still load, after running the
/// migration chain. No migrations exist yet — this equals
/// `CURRENT_SCHEMA_VERSION` because schema version 1 is the first version
/// ever shipped.
const OLDEST_SUPPORTED_VERSION: u32 = 1;

#[derive(Debug, Error)]
pub enum MigrationError {
    #[error("missing or invalid schema_version field")]
    MissingVersion,
    #[error("schema_version {found} predates the oldest version this build supports ({oldest})")]
    UnsupportedVersion { found: u32, oldest: u32 },
    #[error("failed to parse snapshot: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Result of loading a snapshot: the parsed snapshot, plus a non-fatal
/// warning if it was written by a newer schema version than this build
/// fully understands (decision D14, §11.3).
#[derive(Debug)]
pub struct LoadedSnapshot {
    pub snapshot: Snapshot,
    pub forward_compat_warning: Option<String>,
}

/// Loads a snapshot from raw JSON, running the backward-compat migration
/// chain in memory — the stored file is never rewritten (§11.3) — and
/// tolerating forward-compat cases: a newer schema version, or unknown
/// fields/enum values within an otherwise-understood version (D14).
pub fn load_snapshot_json(raw: &str) -> Result<LoadedSnapshot, MigrationError> {
    let value: serde_json::Value = serde_json::from_str(raw)?;
    let schema_version = value
        .get("schema_version")
        .and_then(|v| v.as_u64())
        .ok_or(MigrationError::MissingVersion)? as u32;

    if schema_version < OLDEST_SUPPORTED_VERSION {
        return Err(MigrationError::UnsupportedVersion {
            found: schema_version,
            oldest: OLDEST_SUPPORTED_VERSION,
        });
    }

    // No migrations exist yet. Future schema bumps add arms here, e.g.:
    //   let value = if schema_version < 2 { migrate_v1_to_v2(value) } else { value };
    let migrated = value;

    let snapshot: Snapshot = serde_json::from_value(migrated)?;

    let forward_compat_warning = if schema_version > CURRENT_SCHEMA_VERSION {
        Some(format!(
            "snapshot was written by a newer schema version ({schema_version}) than this build fully understands ({CURRENT_SCHEMA_VERSION}); some fields may have been skipped"
        ))
    } else {
        None
    };

    Ok(LoadedSnapshot {
        snapshot,
        forward_compat_warning,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const V1_FIXTURE: &str = r#"
    {
      "schema_version": 1,
      "snapshot_id": "b3f1-test",
      "created_at": "2026-06-30T09:15:00-07:00",
      "accounts": [
        {
          "account_key": "sha256:9f2a...",
          "source_id": "chase_checking",
          "institution": "Chase",
          "category": "asset",
          "type": "checking",
          "balance": 4213.55,
          "currency": "USD",
          "status": "ok"
        }
      ]
    }
    "#;

    #[test]
    fn v1_fixture_loads_correctly() {
        let loaded = load_snapshot_json(V1_FIXTURE).unwrap();
        assert_eq!(loaded.snapshot.schema_version, 1);
        assert_eq!(loaded.snapshot.accounts.len(), 1);
        assert!(loaded.forward_compat_warning.is_none());
    }

    #[test]
    fn unknown_top_level_field_is_ignored() {
        let fixture = r#"
        {
          "schema_version": 1,
          "snapshot_id": "b3f1-test",
          "created_at": "2026-06-30T09:15:00-07:00",
          "future_top_level_field": "something added later",
          "accounts": []
        }
        "#;
        let loaded = load_snapshot_json(fixture).unwrap();
        assert_eq!(loaded.snapshot.accounts.len(), 0);
        assert!(loaded.forward_compat_warning.is_none());
    }

    #[test]
    fn newer_schema_version_loads_leniently_with_warning() {
        let fixture = r#"
        {
          "schema_version": 2,
          "snapshot_id": "b3f1-test",
          "created_at": "2026-06-30T09:15:00-07:00",
          "accounts": []
        }
        "#;
        let loaded = load_snapshot_json(fixture).unwrap();
        assert_eq!(loaded.snapshot.schema_version, 2);
        assert!(loaded.forward_compat_warning.is_some());
    }

    #[test]
    fn unknown_category_value_falls_back_to_unknown_variant() {
        let fixture = r#"
        {
          "schema_version": 2,
          "snapshot_id": "b3f1-test",
          "created_at": "2026-06-30T09:15:00-07:00",
          "accounts": [
            {
              "account_key": "sha256:aaaa...",
              "source_id": "future_source",
              "institution": "SomeExchange",
              "category": "cryptocurrency",
              "type": "wallet",
              "balance": 100.0,
              "currency": "USD",
              "status": "ok"
            }
          ]
        }
        "#;
        let loaded = load_snapshot_json(fixture).unwrap();
        assert_eq!(
            loaded.snapshot.accounts[0].category,
            crate::Category::Unknown
        );
    }

    #[test]
    fn unknown_status_value_falls_back_to_unknown_variant() {
        let fixture = r#"
        {
          "schema_version": 2,
          "snapshot_id": "b3f1-test",
          "created_at": "2026-06-30T09:15:00-07:00",
          "accounts": [
            {
              "account_key": "sha256:bbbb...",
              "source_id": "future_source",
              "institution": "SomeExchange",
              "category": "asset",
              "type": "wallet",
              "balance": 100.0,
              "currency": "USD",
              "status": "pending_future_status"
            }
          ]
        }
        "#;
        let loaded = load_snapshot_json(fixture).unwrap();
        assert_eq!(loaded.snapshot.accounts[0].status, crate::Status::Unknown);
    }

    #[test]
    fn missing_schema_version_is_an_error() {
        let fixture = r#"{ "accounts": [] }"#;
        let err = load_snapshot_json(fixture).unwrap_err();
        assert!(matches!(err, MigrationError::MissingVersion));
    }

    #[test]
    fn schema_version_below_oldest_supported_is_an_error() {
        let fixture = r#"{ "schema_version": 0, "accounts": [] }"#;
        let err = load_snapshot_json(fixture).unwrap_err();
        assert!(matches!(err, MigrationError::UnsupportedVersion { .. }));
    }
}
