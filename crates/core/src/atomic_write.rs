//! The one atomic write shared by every storage module added for D44
//! (spec §4): write a sibling temp file with `0600` permissions, then rename
//! it over the real path, so a crash mid-write can never leave a
//! half-written file in place. A missing parent directory is created `0700`.
//!
//! `storage.rs`, `rules_storage.rs` and `sources.rs` each still carry their
//! own copy of this. Consolidating them waits for #37, which changes two of
//! those files.

use std::fs;
use std::io::{self, Write};
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

// Its callers are the storage modules that stream tasks 112, 114 and 115
// implement; until they land it's only reached from its own tests.
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) fn write_atomically(path: &Path, bytes: &[u8]) -> io::Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        // Locked down only when created here: an existing parent may be a
        // directory that isn't this app's to chmod.
        if !parent.exists() {
            fs::create_dir_all(parent)?;
            fs::set_permissions(parent, fs::Permissions::from_mode(0o700))?;
        }
    }

    let mut temp_name = path
        .file_name()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path has no file name"))?
        .to_os_string();
    temp_name.push(".tmp");
    let temp_path = path.with_file_name(temp_name);

    {
        let mut file = fs::File::create(&temp_path)?;
        file.set_permissions(fs::Permissions::from_mode(0o600))?;
        file.write_all(bytes)?;
        file.sync_all()?;
    }
    fs::rename(&temp_path, path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "obol-atomic-write-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn mode(path: &Path) -> u32 {
        fs::metadata(path).unwrap().permissions().mode() & 0o777
    }

    #[test]
    fn should_write_the_bytes_to_the_path() {
        let path = temp_root("writes").join("out.json");

        write_atomically(&path, b"hello").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"hello");
    }

    #[test]
    fn should_create_the_file_with_0600_permissions() {
        let path = temp_root("file-perms").join("out.json");

        write_atomically(&path, b"x").unwrap();

        assert_eq!(mode(&path), 0o600);
    }

    #[test]
    fn should_create_a_missing_parent_directory_with_0700_permissions() {
        let parent = temp_root("dir-perms").join("transactions");

        write_atomically(&parent.join("2026-10.json"), b"x").unwrap();

        assert_eq!(mode(&parent), 0o700);
    }

    #[test]
    fn should_not_change_the_permissions_of_an_existing_parent() {
        // Only a directory this function created is its to lock down —
        // chmod-ing an existing parent could be something like a shared
        // temp directory.
        let parent = temp_root("existing-parent");
        fs::set_permissions(&parent, fs::Permissions::from_mode(0o755)).unwrap();

        write_atomically(&parent.join("out.json"), b"x").unwrap();

        assert_eq!(mode(&parent), 0o755);
    }

    #[test]
    fn should_replace_an_existing_file() {
        let path = temp_root("replaces").join("out.json");
        write_atomically(&path, b"first").unwrap();

        write_atomically(&path, b"second").unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"second");
    }

    #[test]
    fn should_leave_no_temp_file_behind() {
        let root = temp_root("no-temp");

        write_atomically(&root.join("out.json"), b"x").unwrap();

        let names: Vec<String> = fs::read_dir(&root)
            .unwrap()
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(names, ["out.json"]);
    }
}
