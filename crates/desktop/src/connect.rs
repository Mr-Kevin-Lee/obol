//! Connecting SimpleFIN from inside the app, replacing `obol link
//! simplefin` for most people: paste a Setup Token, review one table of
//! accounts with obol's guesses already filled in, press Link.
//!
//! **Contract.** Signatures and types are final; bodies are agent A's.

use std::collections::BTreeSet;

use obol_core::{AccountSet, SourceConfig};
use serde::{Deserialize, Serialize};

use crate::dashboard::StorageLayout;
use crate::simplefin::SimpleFin;

/// Only the two sides a source can be on; core's `Category` also has
/// `Unknown`, which the form must never offer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    Asset,
    Liability,
}

/// One row of the connect form. A preview fills it with obol's guesses;
/// the webview sends it back with the user's choices to link.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ConnectAccount {
    pub account_id: String,
    pub institution: String,
    pub name: String,
    /// SimpleFIN's numeric string, shown so the account is recognizable.
    pub balance: String,
    pub side: Side,
    /// Lowercase snake_case, e.g. `checking`, `credit_card`.
    pub account_type: String,
    /// Ticked in the form. A preview leaves an earlier-ignored account
    /// unticked and every other one ticked.
    pub include: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConnectPreview {
    /// Every Bridge account not yet linked to a source, in Bridge order.
    pub accounts: Vec<ConnectAccount>,
    /// The Bridge's own error messages (an institution needing re-auth,
    /// say), sanitized, so the form can show why an account is missing.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct LinkResult {
    pub linked: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub struct ConnectStatus {
    /// How many sources in `sources.yaml` use SimpleFIN. Zero means the
    /// Net worth screen should offer to connect.
    pub simplefin_sources: usize,
}

/// Reads `sources.yaml` only: no Keychain, no network.
pub fn connect_status(_layout: &StorageLayout) -> Result<ConnectStatus, String> {
    todo!("agent A")
}

/// The form's rows for `set`: every account no existing source links,
/// with obol's guess (`simplefin::guess`), ticked unless in `ignored`.
pub fn preview_rows(
    _set: &AccountSet,
    _existing: &[SourceConfig],
    _ignored: &BTreeSet<String>,
) -> ConnectPreview {
    todo!("agent A")
}

/// With a Setup Token: checks that the Keychain accepts a write first
/// (claiming is one-time, so a failed save would waste the token), claims
/// it, and stores the Access URL. Without one: reuses the stored Access
/// URL. Then fetches the accounts through the shared provider and
/// previews them.
pub async fn preview(
    _layout: &StorageLayout,
    _simplefin: &SimpleFin,
    _setup_token: Option<String>,
) -> Result<ConnectPreview, String> {
    todo!("agent A")
}

/// Adds a source for each included account and records each excluded one
/// as ignored; an included account is removed from the ignore list.
pub fn link_accounts(
    _layout: &StorageLayout,
    _accounts: &[ConnectAccount],
) -> Result<LinkResult, String> {
    todo!("agent A")
}
