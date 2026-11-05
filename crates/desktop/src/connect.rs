//! Connecting SimpleFIN from inside the app, replacing `obol link
//! simplefin` for most people: paste a Setup Token, review one table of
//! accounts with obol's guesses already filled in, press Link.
//!
//! **Contract.** Signatures and types are final; bodies are agent A's.

use std::collections::BTreeSet;
use std::sync::Arc;

use obol_core::{AccountSet, Category, LinkableAccount, SourceConfig};
use serde::{Deserialize, Serialize};

use crate::dashboard::StorageLayout;
use crate::simplefin::{self, SimpleFin};

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
pub fn connect_status(layout: &StorageLayout) -> Result<ConnectStatus, String> {
    let sources = read_sources(layout)?;
    Ok(ConnectStatus {
        simplefin_sources: sources.iter().filter(|s| s.provider == "simplefin").count(),
    })
}

/// The form's rows for `set`: every account no existing source links,
/// with obol's guess (`simplefin::guess`), ticked unless in `ignored`.
pub fn preview_rows(
    set: &AccountSet,
    existing: &[SourceConfig],
    ignored: &BTreeSet<String>,
) -> ConnectPreview {
    let accounts = obol_core::linkable_accounts(set, existing)
        .into_iter()
        .map(|account| {
            let (category, account_type) = simplefin::guess(&account);
            ConnectAccount {
                include: !ignored.contains(&account.account_id),
                side: side(category),
                account_type: account_type.to_string(),
                account_id: account.account_id,
                institution: account.institution,
                name: account.name,
                balance: account.balance,
            }
        })
        .collect();
    ConnectPreview {
        accounts,
        warnings: set
            .errlist
            .iter()
            .map(|e| obol_core::sanitize_server_message(&e.msg))
            .collect(),
    }
}

/// With a Setup Token: checks that the Keychain accepts a write first
/// (claiming is one-time, so a failed save would waste the token), claims
/// it, and stores the Access URL. Without one: reuses the stored Access
/// URL. Then fetches the accounts through the shared provider and
/// previews them.
pub async fn preview(
    layout: &StorageLayout,
    simplefin: &SimpleFin,
    setup_token: Option<String>,
) -> Result<ConnectPreview, String> {
    // Local files first, so a broken one can't waste a one-time claim.
    let existing = read_sources(layout)?;
    let ignored = simplefin::read_ignored(layout)?;
    let set = fetch_accounts(simplefin, setup_token).await?;
    Ok(preview_rows(&set, &existing, &ignored))
}

/// Adds a source for each included account and records each excluded one
/// as ignored; an included account is removed from the ignore list.
pub fn link_accounts(
    layout: &StorageLayout,
    accounts: &[ConnectAccount],
) -> Result<LinkResult, String> {
    let mut sources = obol_core::load_or_init(&layout.sources).map_err(sources_error)?;
    let before = simplefin::read_ignored(layout)?;
    let mut ignored = before.clone();
    for row in accounts {
        if row.include {
            ignored.remove(&row.account_id);
        } else {
            ignored.insert(row.account_id.clone());
        }
    }
    if ignored != before {
        simplefin::write_ignored(layout, &ignored)?;
    }

    let mut linked = 0;
    for row in accounts.iter().filter(|row| row.include) {
        if is_linked(&sources, &row.account_id) {
            continue;
        }
        let source = source_for(row, &sources);
        obol_core::add_source(&layout.sources, source.clone())
            .map_err(|e| format!("{} could not be linked: {e}", row.name))?;
        sources.push(source);
        linked += 1;
    }
    Ok(LinkResult { linked })
}

/// Like `load_or_init`, minus the init: a read never creates files.
fn read_sources(layout: &StorageLayout) -> Result<Vec<SourceConfig>, String> {
    if !layout.sources.exists() {
        return Ok(Vec::new());
    }
    obol_core::load_or_init(&layout.sources).map_err(sources_error)
}

fn sources_error(err: obol_core::SourcesError) -> String {
    format!("sources.yaml could not be loaded: {err}")
}

/// The Access URL never leaves this function; error messages carry only
/// core's errors, which never include it.
async fn fetch_accounts(
    simplefin: &SimpleFin,
    pasted: Option<String>,
) -> Result<Arc<AccountSet>, String> {
    let token = setup_token(pasted);
    let claimed = token.is_some();
    // The UI keeps the pasted token only after the two failures worded
    // "wasn't used" and "didn't accept" (ConnectSimplefin.svelte), since
    // every later failure means the one-time token is already spent.
    let access_url = match token {
        Some(token) => {
            obol_core::probe_keychain_writable().map_err(|e| {
                format!(
                    "The Keychain refused a test save ({e}), so the Setup Token wasn't used: \
                     it works only once, and the connection couldn't be saved."
                )
            })?;
            let url = obol_core::SimpleFinClient::default()
                .claim(&token)
                .await
                .map_err(|e| format!("SimpleFIN didn't accept the Setup Token: {e}."))?;
            obol_core::store_simplefin_access_url(&url).map_err(|e| {
                format!(
                    "The Setup Token was used, but saving the connection to the Keychain \
                     failed ({e}). Create a new Setup Token and try again."
                )
            })?;
            simplefin.remember(url.clone()).await;
            url
        }
        None => saved_access_url(simplefin.access_url().await)?,
    };
    simplefin
        .provider
        .account_set(&access_url)
        .await
        .map_err(|e| {
            if claimed {
                format!(
                    "SimpleFIN is connected, but your accounts couldn't be fetched: {e}. \
                     Try again with your saved connection."
                )
            } else {
                format!("Your accounts couldn't be fetched from SimpleFIN: {e}.")
            }
        })
}

/// A blank paste means "reuse the saved connection".
fn setup_token(token: Option<String>) -> Option<String> {
    token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
}

fn saved_access_url<S>(stored: Option<S>) -> Result<S, String> {
    stored.ok_or_else(|| {
        "SimpleFIN isn't connected yet. Paste a Setup Token to connect it.".to_string()
    })
}

fn side(category: Category) -> Side {
    match category {
        Category::Liability => Side::Liability,
        _ => Side::Asset,
    }
}

fn is_linked(sources: &[SourceConfig], account_id: &str) -> bool {
    sources.iter().any(|s| {
        s.provider == "simplefin"
            && s.provider_config
                .get("simplefin_account_id")
                .and_then(|v| v.as_str())
                == Some(account_id)
    })
}

fn source_for(row: &ConnectAccount, existing: &[SourceConfig]) -> SourceConfig {
    let category = match row.side {
        Side::Asset => Category::Asset,
        Side::Liability => Category::Liability,
    };
    let typed = obol_core::normalize_account_type(&row.account_type);
    let account_type = if typed.is_empty() {
        obol_core::default_account_type(&row.name, category).to_string()
    } else {
        typed
    };
    let account = LinkableAccount {
        account_id: row.account_id.clone(),
        name: row.name.clone(),
        institution: row.institution.clone(),
        balance: row.balance.clone(),
    };
    obol_core::new_source(&account, category, &account_type, existing)
}

#[cfg(test)]
mod tests {
    use super::*;
    use obol_core::{SimpleFinAccount, SimpleFinApiError, SimpleFinConnection};
    use serde_json::json;
    use std::fs;
    use std::path::PathBuf;

    fn temp_layout(name: &str) -> StorageLayout {
        let root: PathBuf = std::env::temp_dir().join(format!(
            "obol-desktop-connect-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        StorageLayout::under(&root)
    }

    fn set_with(accounts: &[(&str, &str, &str)]) -> AccountSet {
        AccountSet {
            errlist: vec![],
            connections: vec![SimpleFinConnection {
                conn_id: "c1".into(),
                name: "Chase Bank".into(),
            }],
            accounts: accounts
                .iter()
                .map(|(id, name, balance)| SimpleFinAccount {
                    id: id.to_string(),
                    name: name.to_string(),
                    conn_id: "c1".into(),
                    balance: balance.to_string(),
                    transactions: vec![],
                })
                .collect(),
        }
    }

    fn error(msg: &str) -> SimpleFinApiError {
        SimpleFinApiError {
            code: "con.auth".into(),
            msg: msg.into(),
            conn_id: Some("c1".into()),
            account_id: None,
        }
    }

    fn source(id: &str, provider: &str, account_id: &str) -> SourceConfig {
        SourceConfig {
            id: id.into(),
            provider: provider.into(),
            category: Category::Asset,
            account_type: "checking".into(),
            institution: "Chase Bank".into(),
            account_salt: String::new(),
            provider_config: json!({ "simplefin_account_id": account_id }),
        }
    }

    fn row(account_id: &str, name: &str, side: Side, account_type: &str) -> ConnectAccount {
        ConnectAccount {
            account_id: account_id.into(),
            institution: "Chase Bank".into(),
            name: name.into(),
            balance: "10.00".into(),
            side,
            account_type: account_type.into(),
            include: true,
        }
    }

    fn excluded(account_id: &str) -> ConnectAccount {
        ConnectAccount {
            include: false,
            ..row(account_id, "Savings", Side::Asset, "savings")
        }
    }

    fn ignored(ids: &[&str]) -> BTreeSet<String> {
        ids.iter().map(|id| id.to_string()).collect()
    }

    fn saved_sources(layout: &StorageLayout) -> Vec<SourceConfig> {
        obol_core::load_or_init(&layout.sources).unwrap()
    }

    fn malformed_sources(layout: &StorageLayout) {
        fs::write(&layout.sources, "sources: [unclosed").unwrap();
    }

    // connect_status

    #[test]
    fn should_count_no_sources_when_the_file_is_missing() {
        let layout = temp_layout("status-missing");

        assert_eq!(
            connect_status(&layout),
            Ok(ConnectStatus {
                simplefin_sources: 0
            })
        );
    }

    #[test]
    fn should_not_create_sources_yaml_when_reading_the_status() {
        let layout = temp_layout("status-no-create");

        let _ = connect_status(&layout);

        assert!(!layout.sources.exists());
    }

    #[test]
    fn should_count_only_simplefin_sources() {
        let layout = temp_layout("status-count");
        obol_core::add_source(&layout.sources, source("a", "simplefin", "ACT-1")).unwrap();
        obol_core::add_source(&layout.sources, source("b", "plaid", "ACT-2")).unwrap();
        obol_core::add_source(&layout.sources, source("c", "simplefin", "ACT-3")).unwrap();

        assert_eq!(connect_status(&layout).unwrap().simplefin_sources, 2);
    }

    #[test]
    fn should_report_a_malformed_sources_file_in_the_status() {
        let layout = temp_layout("status-malformed");
        malformed_sources(&layout);

        assert!(connect_status(&layout).is_err());
    }

    // preview_rows

    #[test]
    fn should_preview_every_unlinked_account_in_bridge_order() {
        let set = set_with(&[
            ("ACT-3", "Card", "-5.00"),
            ("ACT-1", "Checking", "10.00"),
            ("ACT-2", "Savings", "20.00"),
        ]);
        let existing = [source("chase", "simplefin", "ACT-1")];

        let preview = preview_rows(&set, &existing, &BTreeSet::new());

        let ids: Vec<&str> = preview
            .accounts
            .iter()
            .map(|a| a.account_id.as_str())
            .collect();
        assert_eq!(ids, ["ACT-3", "ACT-2"]);
    }

    #[test]
    fn should_fill_a_row_with_the_account_and_obols_guess() {
        let set = set_with(&[("ACT-1", "Total Checking", "2400.00")]);

        let preview = preview_rows(&set, &[], &BTreeSet::new());

        assert_eq!(
            preview.accounts,
            [ConnectAccount {
                account_id: "ACT-1".into(),
                institution: "Chase Bank".into(),
                name: "Total Checking".into(),
                balance: "2400.00".into(),
                side: Side::Asset,
                account_type: "checking".into(),
                include: true,
            }]
        );
    }

    #[test]
    fn should_guess_a_liability_for_a_negative_balance() {
        let set = set_with(&[("ACT-1", "Sapphire Card", "-120.50")]);

        let preview = preview_rows(&set, &[], &BTreeSet::new());

        assert_eq!(preview.accounts[0].side, Side::Liability);
    }

    #[test]
    fn should_leave_an_ignored_account_unticked() {
        let set = set_with(&[("ACT-1", "Checking", "10.00")]);

        let preview = preview_rows(&set, &[], &ignored(&["ACT-1"]));

        assert!(!preview.accounts[0].include);
    }

    #[test]
    fn should_sanitize_one_warning_per_bridge_error() {
        let mut set = set_with(&[]);
        set.errlist = vec![error("Re-auth\u{1b}[2J needed"), error("Bank down")];

        let preview = preview_rows(&set, &[], &BTreeSet::new());

        assert_eq!(preview.warnings, ["Re-auth[2J needed", "Bank down"]);
    }

    #[test]
    fn should_put_an_unknown_category_on_the_asset_side() {
        assert_eq!(side(Category::Unknown), Side::Asset);
    }

    // preview

    #[tokio::test]
    async fn should_report_a_malformed_sources_file_before_claiming_the_token() {
        let layout = temp_layout("preview-malformed-sources");
        malformed_sources(&layout);
        // Also broken, so a mutant that skips the sources check still
        // stops before the Keychain.
        fs::write(&layout.simplefin_ignored, "not json").unwrap();
        let simplefin = SimpleFin::new(layout.simplefin_usage.clone());

        let result = preview(&layout, &simplefin, Some("not-a-real-token".into())).await;

        assert!(result.unwrap_err().contains("sources.yaml"));
    }

    #[tokio::test]
    async fn should_report_a_malformed_ignore_list_before_claiming_the_token() {
        let layout = temp_layout("preview-malformed-ignored");
        fs::write(&layout.simplefin_ignored, "not json").unwrap();
        let simplefin = SimpleFin::new(layout.simplefin_usage.clone());

        let result = preview(&layout, &simplefin, Some("not-a-real-token".into())).await;

        assert!(result.unwrap_err().contains("simplefin_ignored.json"));
    }

    #[tokio::test]
    async fn should_not_create_sources_yaml_when_previewing() {
        let layout = temp_layout("preview-no-create");
        fs::write(&layout.simplefin_ignored, "not json").unwrap();
        let simplefin = SimpleFin::new(layout.simplefin_usage.clone());

        let _ = preview(&layout, &simplefin, None).await;

        assert!(!layout.sources.exists());
    }

    #[test]
    fn should_treat_a_missing_setup_token_as_none() {
        assert_eq!(setup_token(None), None);
    }

    #[test]
    fn should_treat_a_blank_setup_token_as_none() {
        assert_eq!(setup_token(Some(" \n ".into())), None);
    }

    #[test]
    fn should_trim_a_pasted_setup_token() {
        assert_eq!(setup_token(Some(" abc\n".into())), Some("abc".into()));
    }

    #[test]
    fn should_ask_for_a_setup_token_when_no_connection_is_saved() {
        assert!(saved_access_url(None::<String>)
            .unwrap_err()
            .contains("Setup Token"));
    }

    #[test]
    fn should_use_the_saved_connection() {
        assert_eq!(saved_access_url(Some("saved")), Ok("saved"));
    }

    // link_accounts

    #[test]
    fn should_return_how_many_accounts_were_linked() {
        let layout = temp_layout("link-count");
        let rows = [
            row("ACT-1", "Checking", Side::Asset, "checking"),
            row("ACT-2", "Card", Side::Liability, "credit_card"),
            excluded("ACT-3"),
        ];

        assert_eq!(link_accounts(&layout, &rows), Ok(LinkResult { linked: 2 }));
    }

    #[test]
    fn should_save_a_source_pointing_at_the_simplefin_account() {
        let layout = temp_layout("link-source");

        link_accounts(
            &layout,
            &[row("ACT-1", "Total Checking", Side::Asset, "checking")],
        )
        .unwrap();

        let saved = saved_sources(&layout);
        assert_eq!(
            (
                saved[0].id.as_str(),
                saved[0].provider.as_str(),
                &saved[0].provider_config
            ),
            (
                "chase_bank_total_checking",
                "simplefin",
                &json!({ "simplefin_account_id": "ACT-1" })
            )
        );
    }

    #[test]
    fn should_save_the_chosen_side() {
        let layout = temp_layout("link-side");

        link_accounts(
            &layout,
            &[row("ACT-1", "Checking", Side::Liability, "loan")],
        )
        .unwrap();

        assert_eq!(saved_sources(&layout)[0].category, Category::Liability);
    }

    #[test]
    fn should_save_the_chosen_asset_side() {
        let layout = temp_layout("link-asset-side");

        link_accounts(&layout, &[row("ACT-1", "Card", Side::Asset, "other")]).unwrap();

        assert_eq!(saved_sources(&layout)[0].category, Category::Asset);
    }

    #[test]
    fn should_normalize_a_typed_account_type() {
        let layout = temp_layout("link-normalize");

        link_accounts(
            &layout,
            &[row("ACT-1", "Vault", Side::Asset, " Money Market ")],
        )
        .unwrap();

        assert_eq!(saved_sources(&layout)[0].account_type, "money_market");
    }

    #[test]
    fn should_guess_the_type_when_none_was_typed() {
        let layout = temp_layout("link-guess-type");

        link_accounts(&layout, &[row("ACT-1", "Total Checking", Side::Asset, "")]).unwrap();

        assert_eq!(saved_sources(&layout)[0].account_type, "checking");
    }

    #[test]
    fn should_guess_a_type_that_fits_the_chosen_side() {
        let layout = temp_layout("link-guess-side-type");

        link_accounts(
            &layout,
            &[row("ACT-1", "Total Checking", Side::Liability, "***")],
        )
        .unwrap();

        assert_eq!(saved_sources(&layout)[0].account_type, "other");
    }

    #[test]
    fn should_not_link_an_excluded_account() {
        let layout = temp_layout("link-excluded");

        link_accounts(&layout, &[excluded("ACT-1")]).unwrap();

        assert!(saved_sources(&layout).is_empty());
    }

    #[test]
    fn should_give_accounts_with_the_same_name_distinct_ids() {
        let layout = temp_layout("link-collision");
        let rows = [
            row("ACT-1", "Checking", Side::Asset, "checking"),
            row("ACT-2", "Checking", Side::Asset, "checking"),
        ];

        link_accounts(&layout, &rows).unwrap();

        let ids: Vec<String> = saved_sources(&layout).into_iter().map(|s| s.id).collect();
        assert_eq!(ids, ["chase_bank_checking", "chase_bank_checking_2"]);
    }

    #[test]
    fn should_not_link_an_account_twice_when_the_form_is_resubmitted() {
        let layout = temp_layout("link-resubmit");
        let rows = [row("ACT-1", "Checking", Side::Asset, "checking")];
        link_accounts(&layout, &rows).unwrap();

        assert_eq!(link_accounts(&layout, &rows), Ok(LinkResult { linked: 0 }));
    }

    #[test]
    fn should_link_an_account_another_provider_uses_the_same_id_for() {
        let layout = temp_layout("link-other-provider");
        obol_core::add_source(&layout.sources, source("plaid", "plaid", "ACT-1")).unwrap();

        let result = link_accounts(
            &layout,
            &[row("ACT-1", "Checking", Side::Asset, "checking")],
        );

        assert_eq!(result, Ok(LinkResult { linked: 1 }));
    }

    #[test]
    fn should_ignore_an_excluded_account() {
        let layout = temp_layout("link-ignore");

        link_accounts(&layout, &[excluded("ACT-1")]).unwrap();

        assert_eq!(simplefin::read_ignored(&layout), Ok(ignored(&["ACT-1"])));
    }

    #[test]
    fn should_stop_ignoring_an_included_account() {
        let layout = temp_layout("link-unignore");
        simplefin::write_ignored(&layout, &ignored(&["ACT-1", "ACT-2"])).unwrap();

        link_accounts(
            &layout,
            &[row("ACT-1", "Checking", Side::Asset, "checking")],
        )
        .unwrap();

        assert_eq!(simplefin::read_ignored(&layout), Ok(ignored(&["ACT-2"])));
    }

    #[test]
    fn should_not_write_the_ignore_list_when_it_is_unchanged() {
        let layout = temp_layout("link-ignore-unchanged");

        link_accounts(
            &layout,
            &[row("ACT-1", "Checking", Side::Asset, "checking")],
        )
        .unwrap();

        assert!(!layout.simplefin_ignored.exists());
    }

    #[test]
    fn should_report_a_malformed_sources_file_when_linking() {
        let layout = temp_layout("link-malformed-sources");
        malformed_sources(&layout);

        // Excluded only, so the error can't come from `add_source` instead.
        assert!(link_accounts(&layout, &[excluded("ACT-1")]).is_err());
    }

    #[test]
    fn should_report_a_malformed_ignore_list_when_linking() {
        let layout = temp_layout("link-malformed-ignored");
        fs::write(&layout.simplefin_ignored, "not json").unwrap();

        assert!(link_accounts(&layout, &[excluded("ACT-1")]).is_err());
    }
}
