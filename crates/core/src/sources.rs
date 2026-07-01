//! `sources.yaml` CRUD (spec §10.1). The file stays git-friendly and
//! inspectable, but is never hand-edited — add/edit/remove go through
//! these functions, atomic-write (temp file + rename) with `0600`
//! permissions (§4). A file that exists but fails to parse blocks the
//! whole run with a clear message (§9.1) rather than silently falling
//! back to an empty list.

use std::fs;
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

use base64::prelude::*;
use rand::Rng;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::SourceConfig;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct SourcesFile {
    #[serde(default)]
    sources: Vec<SourceConfig>,
}

#[derive(Debug, Error)]
pub enum SourcesError {
    #[error("sources.yaml could not be parsed: {0}")]
    Parse(serde_saphyr::Error),
    #[error("failed to write sources.yaml: {0}")]
    Serialize(serde_saphyr::ser_error::Error),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("no source with id '{0}' found")]
    NotFound(String),
    #[error("a source with id '{0}' already exists")]
    AlreadyExists(String),
}

/// Loads `sources.yaml`, creating an empty one if it doesn't exist yet
/// (§10.1's first-run behavior — the caller is responsible for then
/// branching to the Sources screen rather than rendering an empty
/// dashboard). A file that exists but fails to parse is a whole-run
/// failure (§9.1): returns `Err` with a clear message, never silently
/// falls back to an empty list.
pub fn load_or_init(path: &Path) -> Result<Vec<SourceConfig>, SourcesError> {
    if !path.exists() {
        write_atomically(path, &SourcesFile::default())?;
        return Ok(Vec::new());
    }

    let contents = fs::read_to_string(path)?;
    let parsed: SourcesFile = serde_saphyr::from_str(&contents).map_err(SourcesError::Parse)?;
    Ok(parsed.sources)
}

/// Adds a new source. Always generates a fresh `account_salt` (D15),
/// overwriting whatever the caller passed in — the "generated once, at
/// add-time" invariant is enforced here structurally, not by convention.
pub fn add_source(path: &Path, mut new_source: SourceConfig) -> Result<(), SourcesError> {
    let mut sources = load_or_init(path)?;
    if sources.iter().any(|s| s.id == new_source.id) {
        return Err(SourcesError::AlreadyExists(new_source.id));
    }
    new_source.account_salt = generate_account_salt();
    sources.push(new_source);
    write_atomically(path, &SourcesFile { sources })
}

/// Edits an existing source. Preserves the original `account_salt`
/// regardless of what's on `updated` — the salt is generated once at
/// add-time (D15) and must never change afterward, since that would
/// break run-over-run account tracking for every historical snapshot.
pub fn edit_source(path: &Path, id: &str, mut updated: SourceConfig) -> Result<(), SourcesError> {
    let mut sources = load_or_init(path)?;
    let index = sources
        .iter()
        .position(|s| s.id == id)
        .ok_or_else(|| SourcesError::NotFound(id.to_string()))?;
    updated.account_salt = sources[index].account_salt.clone();
    sources[index] = updated;
    write_atomically(path, &SourcesFile { sources })
}

/// Removes a source by id. Callers are responsible for any
/// provider-specific cleanup that must happen first (e.g. Plaid's
/// `/item/remove` + Keychain deletion, §10.1) — this function only
/// touches `sources.yaml` itself.
pub fn remove_source(path: &Path, id: &str) -> Result<(), SourcesError> {
    let mut sources = load_or_init(path)?;
    let index = sources
        .iter()
        .position(|s| s.id == id)
        .ok_or_else(|| SourcesError::NotFound(id.to_string()))?;
    sources.remove(index);
    write_atomically(path, &SourcesFile { sources })
}

/// A fresh, random salt for a new source (D15) — matches the
/// `"b64:..."` format shown in the spec's `sources.yaml` example.
fn generate_account_salt() -> String {
    let mut bytes = [0u8; 16];
    rand::rng().fill_bytes(&mut bytes);
    format!("b64:{}", BASE64_STANDARD.encode(bytes))
}

/// Atomic write (temp file + rename) with `0600` permissions (§4,
/// §10.1) — a crash mid-write can't corrupt the existing file, since
/// the rename is the only operation that touches the real path.
fn write_atomically(path: &Path, sources_file: &SourcesFile) -> Result<(), SourcesError> {
    let yaml = serde_saphyr::to_string(sources_file).map_err(SourcesError::Serialize)?;

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
    use crate::Category;

    fn fake_source(id: &str) -> SourceConfig {
        SourceConfig {
            id: id.to_string(),
            provider: "plaid".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            institution: "Chase".into(),
            account_salt: "will-be-overwritten".into(),
            provider_config: serde_json::json!({ "plaid_account_id": "acc_123" }),
        }
    }

    fn temp_path(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!(
            "obol-sources-test-{name}-{}.yaml",
            std::process::id()
        ))
    }

    #[test]
    fn load_or_init_creates_an_empty_file_on_first_run() {
        let path = temp_path("first-run");
        let _ = fs::remove_file(&path);

        let sources = load_or_init(&path).unwrap();
        assert!(sources.is_empty());
        assert!(path.exists());

        let perms = fs::metadata(&path).unwrap().permissions();
        assert_eq!(perms.mode() & 0o777, 0o600);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn add_then_load_round_trips() {
        let path = temp_path("add-roundtrip");
        let _ = fs::remove_file(&path);

        add_source(&path, fake_source("chase_checking")).unwrap();
        let sources = load_or_init(&path).unwrap();

        assert_eq!(sources.len(), 1);
        assert_eq!(sources[0].id, "chase_checking");

        fs::remove_file(&path).ok();
    }

    #[test]
    fn add_generates_a_fresh_salt_regardless_of_input() {
        let path = temp_path("add-salt");
        let _ = fs::remove_file(&path);

        add_source(&path, fake_source("chase_checking")).unwrap();
        let sources = load_or_init(&path).unwrap();

        assert_ne!(sources[0].account_salt, "will-be-overwritten");
        assert!(sources[0].account_salt.starts_with("b64:"));

        fs::remove_file(&path).ok();
    }

    #[test]
    fn adding_a_duplicate_id_is_an_error() {
        let path = temp_path("dup");
        let _ = fs::remove_file(&path);

        add_source(&path, fake_source("chase_checking")).unwrap();
        let err = add_source(&path, fake_source("chase_checking")).unwrap_err();
        assert!(matches!(err, SourcesError::AlreadyExists(_)));

        fs::remove_file(&path).ok();
    }

    #[test]
    fn edit_updates_fields_but_preserves_the_original_salt() {
        let path = temp_path("edit");
        let _ = fs::remove_file(&path);

        add_source(&path, fake_source("chase_checking")).unwrap();
        let original_salt = load_or_init(&path).unwrap()[0].account_salt.clone();

        let mut updated = fake_source("chase_checking");
        updated.institution = "Chase Bank".into();
        edit_source(&path, "chase_checking", updated).unwrap();

        let sources = load_or_init(&path).unwrap();
        assert_eq!(sources[0].institution, "Chase Bank");
        assert_eq!(sources[0].account_salt, original_salt);

        fs::remove_file(&path).ok();
    }

    #[test]
    fn editing_a_nonexistent_source_is_an_error() {
        let path = temp_path("edit-missing");
        let _ = fs::remove_file(&path);

        let err = edit_source(&path, "does_not_exist", fake_source("does_not_exist")).unwrap_err();
        assert!(matches!(err, SourcesError::NotFound(_)));

        fs::remove_file(&path).ok();
    }

    #[test]
    fn remove_deletes_the_source() {
        let path = temp_path("remove");
        let _ = fs::remove_file(&path);

        add_source(&path, fake_source("chase_checking")).unwrap();
        remove_source(&path, "chase_checking").unwrap();

        let sources = load_or_init(&path).unwrap();
        assert!(sources.is_empty());

        fs::remove_file(&path).ok();
    }

    #[test]
    fn removing_a_nonexistent_source_is_an_error() {
        let path = temp_path("remove-missing");
        let _ = fs::remove_file(&path);

        let err = remove_source(&path, "does_not_exist").unwrap_err();
        assert!(matches!(err, SourcesError::NotFound(_)));

        fs::remove_file(&path).ok();
    }

    #[test]
    fn a_malformed_file_produces_a_clear_parse_error() {
        let path = temp_path("malformed");
        fs::write(&path, "sources: [this is not valid: yaml: at all: -").unwrap();

        let err = load_or_init(&path).unwrap_err();
        assert!(matches!(err, SourcesError::Parse(_)));
        assert!(err.to_string().contains("could not be parsed"));

        fs::remove_file(&path).ok();
    }
}
