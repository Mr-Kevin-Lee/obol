//! macOS Keychain storage (spec §4, §8, D20). Stores two kinds of
//! secrets, both under this app's own service name so they're inspectable
//! and revocable independent of any real bank password:
//!   - Per-Item Plaid access tokens (§8), keyed by source id.
//!   - This app's own Plaid client_id/secret pair (D20) — a single
//!     app-level credential, not per-source.
//!
//! **Confidence note:** `read`/`delete` use `security-framework`'s
//! high-level `passwords` module, confirmed working against a real
//! Keychain. `store` uses `set_generic_password_options` with a
//! `SecAccessControl` built from `ProtectionMode::
//! AccessibleWhenUnlockedThisDeviceOnly` — confirmed via docs.rs to be
//! the documented, high-level way to set this accessibility level (§4,
//! §8), not a raw-FFI guess. Not calling `set_access_synchronized`
//! explicitly: `ThisDeviceOnly` protection classes are categorically
//! excluded from Keychain sync by Apple's own semantics, so setting it
//! separately would be redundant at best.

use secrecy::{ExposeSecret, Secret};
use security_framework::access_control::{ProtectionMode, SecAccessControl};
use security_framework::passwords::{
    delete_generic_password, get_generic_password, set_generic_password_options, PasswordOptions,
};
use thiserror::Error;

/// This app's Keychain service name — all entries live under this, so
/// they're all inspectable/deletable together and scoped to this app
/// specifically. Revisit if/when real app packaging picks a bundle
/// identifier (task 27) — this is a placeholder until then.
const SERVICE_NAME: &str = "com.obol.plaid";

#[derive(Debug, Error)]
pub enum KeychainError {
    #[error("keychain operation failed: {0}")]
    Keychain(#[from] security_framework::base::Error),
    #[error("stored value was not valid UTF-8")]
    InvalidUtf8,
}

/// Stores (creates or updates) a Keychain item with
/// `kSecAttrAccessibleWhenUnlockedThisDeviceOnly` explicitly set (§4,
/// §8) via `PasswordOptions`/`SecAccessControl` — the documented
/// high-level way to control this attribute, confirmed against docs.rs
/// rather than assumed.
fn store(account: &str, value: &Secret<String>) -> Result<(), KeychainError> {
    let access_control = SecAccessControl::create_with_protection(
        Some(ProtectionMode::AccessibleWhenUnlockedThisDeviceOnly),
        0,
    )?;

    let mut options = PasswordOptions::new_generic_password(SERVICE_NAME, account);
    options.set_access_control(access_control);

    set_generic_password_options(value.expose_secret().as_bytes(), options)?;

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

fn access_token_account(source_id: &str) -> String {
    format!("plaid_access_token:{source_id}")
}

/// Stores a Plaid access token for one source (§8). Overwrites any
/// existing entry for the same `source_id`.
pub fn store_plaid_access_token(
    source_id: &str,
    token: &Secret<String>,
) -> Result<(), KeychainError> {
    store(&access_token_account(source_id), token)
}

pub fn read_plaid_access_token(source_id: &str) -> Result<Secret<String>, KeychainError> {
    read(&access_token_account(source_id))
}

/// Deletes a source's access token — called as part of the confirmed
/// remove flow (§10.1, task 20), after `/item/remove` has already
/// released the Item on Plaid's side.
pub fn delete_plaid_access_token(source_id: &str) -> Result<(), KeychainError> {
    delete(&access_token_account(source_id))
}

/// Stores this app's own Plaid client_id/secret pair (D20) — not a
/// per-source credential, a single app-level entry.
pub fn store_plaid_app_credentials(
    client_id: &Secret<String>,
    secret: &Secret<String>,
) -> Result<(), KeychainError> {
    store("plaid_client_id", client_id)?;
    store("plaid_secret", secret)?;
    Ok(())
}

pub fn read_plaid_app_credentials() -> Result<(Secret<String>, Secret<String>), KeychainError> {
    let client_id = read("plaid_client_id")?;
    let secret = read("plaid_secret")?;
    Ok((client_id, secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    // These need a real Keychain, so they're #[ignore]d like the Plaid
    // Sandbox tests — run explicitly with:
    //   cargo test -p obol-core --lib -- --ignored keychain
    // A macOS permission prompt on first run would be expected/normal
    // for an unsigned dev binary touching Keychain for the first time.

    #[test]
    #[ignore = "requires a real macOS Keychain"]
    fn access_token_round_trips() {
        let source_id = "test_source_keychain_roundtrip";
        let token = Secret::new("test-access-token-12345".to_string());

        store_plaid_access_token(source_id, &token).expect("store should succeed");

        let read_back = read_plaid_access_token(source_id).expect("read should succeed");
        assert_eq!(read_back.expose_secret(), token.expose_secret());

        delete_plaid_access_token(source_id).expect("delete should succeed");

        let after_delete = read_plaid_access_token(source_id);
        assert!(
            after_delete.is_err(),
            "expected an error reading a deleted entry, got Ok"
        );
    }

    #[test]
    #[ignore = "requires a real macOS Keychain"]
    fn app_credentials_round_trip() {
        let client_id = Secret::new("test-client-id".to_string());
        let secret = Secret::new("test-secret".to_string());

        store_plaid_app_credentials(&client_id, &secret).expect("store should succeed");

        let (read_client_id, read_secret) =
            read_plaid_app_credentials().expect("read should succeed");
        assert_eq!(read_client_id.expose_secret(), client_id.expose_secret());
        assert_eq!(read_secret.expose_secret(), secret.expose_secret());

        // Cleanup so repeated runs stay idempotent.
        delete("plaid_client_id").ok();
        delete("plaid_secret").ok();
    }

    #[test]
    #[ignore = "requires a real macOS Keychain"]
    fn reading_a_nonexistent_entry_is_an_error() {
        let result = read_plaid_access_token("definitely_does_not_exist_source_id");
        assert!(result.is_err());
    }
}
