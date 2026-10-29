//! macOS Keychain storage for the SimpleFIN Access URL (spec §4, §8). The
//! URL embeds Basic Auth credentials, so it lives here and never in
//! `sources.yaml`.
//!
//! Writes are a plain generic-password write, without a
//! `SecAccessControl`/ThisDeviceOnly attribute: that attribute failed for
//! unsigned binaries (`errSecMissingEntitlement`, decision D24), and the
//! URL needs to be storable today. The item still lives in the login
//! Keychain. `read`/`delete` never hit that problem.

use secrecy::{ExposeSecret, Secret};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password,
};
use thiserror::Error;

/// Kept as the original service name: items are stored under it, so
/// renaming would orphan an already-linked Access URL.
const SERVICE_NAME: &str = "com.obol.plaid";

/// The one SimpleFIN Access URL (it covers every account, so it isn't
/// keyed by source). It embeds Basic Auth credentials.
const SIMPLEFIN_ACCESS_URL_ACCOUNT: &str = "simplefin_access_url";

#[derive(Debug, Error)]
pub enum KeychainError {
    #[error("keychain operation failed: {0}")]
    Keychain(#[from] security_framework::base::Error),
    #[error("stored value was not valid UTF-8")]
    InvalidUtf8,
}

fn store_plain(account: &str, value: &Secret<String>) -> Result<(), KeychainError> {
    set_generic_password(SERVICE_NAME, account, value.expose_secret().as_bytes())?;
    Ok(())
}

fn read(account: &str) -> Result<Secret<String>, KeychainError> {
    let bytes = get_generic_password(SERVICE_NAME, account)?;
    let s = String::from_utf8(bytes).map_err(|_| KeychainError::InvalidUtf8)?;
    Ok(Secret::new(s))
}

fn delete(account: &str) -> Result<(), KeychainError> {
    delete_generic_password(SERVICE_NAME, account)?;
    Ok(())
}

pub fn store_simplefin_access_url(url: &Secret<String>) -> Result<(), KeychainError> {
    store_plain(SIMPLEFIN_ACCESS_URL_ACCOUNT, url)
}

pub fn read_simplefin_access_url() -> Result<Secret<String>, KeychainError> {
    read(SIMPLEFIN_ACCESS_URL_ACCOUNT)
}

/// Writes and deletes a throwaway item. Linking calls this *before*
/// claiming the Setup Token, because a claim is one-time: if the Keychain
/// then rejected the write, the Access URL would be lost.
pub fn probe_keychain_writable() -> Result<(), KeychainError> {
    const PROBE_ACCOUNT: &str = "write_probe";
    store_plain(PROBE_ACCOUNT, &Secret::new("probe".to_string()))?;
    delete(PROBE_ACCOUNT)
}

#[cfg(test)]
mod tests {
    use super::*;

    // These need a real Keychain, so they're #[ignore]d. Run them with
    // `cargo test -p obol-core --lib -- --ignored keychain`.

    // A throwaway account name, so this can never touch a real stored URL.
    #[test]
    #[ignore = "requires a real macOS Keychain"]
    fn plain_store_round_trips_unsigned() {
        let account = "simplefin_access_url_test";
        let url = Secret::new("https://user:pass@bridge.example/simplefin".to_string());

        store_plain(account, &url).expect("plain store should succeed unsigned");

        let read_back = read(account).expect("read should succeed");
        assert_eq!(read_back.expose_secret(), url.expose_secret());

        delete(account).expect("delete should succeed");
        assert!(read(account).is_err());
    }

    #[test]
    #[ignore = "requires a real macOS Keychain"]
    fn probe_reports_a_writable_keychain() {
        probe_keychain_writable().expect("probe should succeed");
    }

    #[test]
    #[ignore = "requires a real macOS Keychain"]
    fn reading_a_nonexistent_entry_is_an_error() {
        assert!(read("definitely_does_not_exist_account").is_err());
    }
}
