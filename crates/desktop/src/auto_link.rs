//! New SimpleFIN accounts join on Refresh: open a new card at a bank
//! that's already connected, and it shows up on the next Refresh with
//! obol's guesses, no relinking. Refresh's own request supplies the
//! accounts (`sync.rs`).

use obol_core::{add_source, linkable_accounts, load_or_init, new_source, AccountSet};

use crate::dashboard::StorageLayout;
use crate::simplefin;

/// Links every account in `set` that's neither linked nor ignored, with
/// obol's guess (`simplefin::guess`), and returns one label per account
/// added, `"<institution> <name>"`, for the Refresh banner.
pub fn auto_link_new_accounts(
    layout: &StorageLayout,
    set: &AccountSet,
) -> Result<Vec<String>, String> {
    let mut sources = load_or_init(&layout.sources)
        .map_err(|err| format!("New SimpleFIN accounts couldn't be checked: {err}"))?;
    let ignored = simplefin::read_ignored(layout)?;
    let mut added = Vec::new();
    for account in linkable_accounts(set, &sources)
        .into_iter()
        .filter(|a| !ignored.contains(&a.account_id))
    {
        let label = format!("{} {}", account.institution, account.name);
        let (category, account_type) = simplefin::guess(&account);
        // Built against the in-memory list so this run's ids don't collide.
        let source = new_source(&account, category, account_type, &sources);
        add_source(&layout.sources, source.clone())
            .map_err(|err| format!("{label} couldn't be linked: {err}"))?;
        sources.push(source);
        added.push(label);
    }
    Ok(added)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simplefin::write_ignored;
    use obol_core::{Category, SimpleFinAccount, SimpleFinConnection, SourceConfig};
    use serde_json::json;
    use std::fs;
    use std::os::unix::fs::{MetadataExt, PermissionsExt};

    fn temp_layout(name: &str) -> StorageLayout {
        let root = std::env::temp_dir().join(format!(
            "obol-desktop-auto-link-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        StorageLayout::under(&root)
    }

    /// `(id, name, balance)`, all at Chase Bank.
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

    /// One SimpleFIN source already linked to `ACT-1`.
    fn linked_layout(name: &str) -> StorageLayout {
        let layout = temp_layout(name);
        add_source(
            &layout.sources,
            source("chase_bank_total_checking", "simplefin", "ACT-1"),
        )
        .unwrap();
        layout
    }

    fn sources(layout: &StorageLayout) -> Vec<SourceConfig> {
        load_or_init(&layout.sources).unwrap()
    }

    fn linked_to(layout: &StorageLayout, account_id: &str) -> SourceConfig {
        sources(layout)
            .into_iter()
            .find(|s| s.provider_config["simplefin_account_id"] == account_id)
            .unwrap()
    }

    #[test]
    fn should_add_nothing_when_every_account_is_linked() {
        let layout = linked_layout("nothing-new");

        let added = auto_link_new_accounts(&layout, &set_with(&[("ACT-1", "Checking", "1")]));

        assert_eq!(added, Ok(vec![]));
    }

    #[test]
    fn should_not_rewrite_sources_when_nothing_is_new() {
        let layout = linked_layout("no-write");
        let before = fs::metadata(&layout.sources).unwrap().ino();

        auto_link_new_accounts(&layout, &set_with(&[("ACT-1", "Checking", "1")])).unwrap();

        assert_eq!(fs::metadata(&layout.sources).unwrap().ino(), before);
    }

    #[test]
    fn should_link_a_new_account() {
        let layout = linked_layout("one-new");

        auto_link_new_accounts(
            &layout,
            &set_with(&[("ACT-1", "Checking", "1"), ("ACT-2", "Savings", "5")]),
        )
        .unwrap();

        assert_eq!(linked_to(&layout, "ACT-2").id, "chase_bank_savings");
    }

    #[test]
    fn should_keep_the_already_linked_source_once() {
        let layout = linked_layout("already-linked");

        auto_link_new_accounts(
            &layout,
            &set_with(&[("ACT-1", "Checking", "1"), ("ACT-2", "Savings", "5")]),
        )
        .unwrap();

        assert_eq!(sources(&layout).len(), 2);
    }

    #[test]
    fn should_salt_the_new_source() {
        let layout = linked_layout("salted");

        auto_link_new_accounts(&layout, &set_with(&[("ACT-2", "Savings", "5")])).unwrap();

        assert!(!linked_to(&layout, "ACT-2").account_salt.is_empty());
    }

    #[test]
    fn should_return_a_label_per_added_account_in_order() {
        let layout = linked_layout("labels");

        let added = auto_link_new_accounts(
            &layout,
            &set_with(&[
                ("ACT-2", "Savings", "5"),
                ("ACT-1", "Checking", "1"),
                ("ACT-3", "Sapphire Card", "-20"),
            ]),
        );

        assert_eq!(
            added,
            Ok(vec![
                "Chase Bank Savings".to_string(),
                "Chase Bank Sapphire Card".to_string()
            ])
        );
    }

    #[test]
    fn should_skip_ignored_accounts() {
        let layout = linked_layout("ignored");
        write_ignored(&layout, &["ACT-2".to_string()].into()).unwrap();

        let added = auto_link_new_accounts(
            &layout,
            &set_with(&[("ACT-2", "Savings", "5"), ("ACT-3", "Brokerage", "9")]),
        );

        assert_eq!(added, Ok(vec!["Chase Bank Brokerage".to_string()]));
    }

    #[test]
    fn should_give_colliding_accounts_distinct_ids() {
        let layout = linked_layout("collide");

        auto_link_new_accounts(
            &layout,
            &set_with(&[("ACT-2", "Card", "-1"), ("ACT-3", "Card", "-2")]),
        )
        .unwrap();

        assert_eq!(linked_to(&layout, "ACT-3").id, "chase_bank_card_2");
    }

    #[test]
    fn should_link_a_negative_balance_as_a_liability() {
        let layout = linked_layout("liability");

        auto_link_new_accounts(&layout, &set_with(&[("ACT-2", "Sapphire Card", "-20")])).unwrap();

        assert_eq!(linked_to(&layout, "ACT-2").category, Category::Liability);
    }

    #[test]
    fn should_link_with_the_guessed_type() {
        let layout = linked_layout("type");

        auto_link_new_accounts(&layout, &set_with(&[("ACT-2", "Sapphire Card", "-20")])).unwrap();

        assert_eq!(linked_to(&layout, "ACT-2").account_type, "credit_card");
    }

    #[test]
    fn should_report_a_malformed_sources_file() {
        let layout = temp_layout("malformed-sources");
        fs::write(&layout.sources, "sources: [not: [valid").unwrap();

        let added = auto_link_new_accounts(&layout, &set_with(&[("ACT-2", "Savings", "5")]));

        assert!(added.unwrap_err().contains("sources.yaml"));
    }

    #[test]
    fn should_report_a_malformed_ignored_file() {
        let layout = linked_layout("malformed-ignored");
        fs::write(&layout.simplefin_ignored, "not json").unwrap();

        let added = auto_link_new_accounts(&layout, &set_with(&[("ACT-2", "Savings", "5")]));

        assert!(added.unwrap_err().contains("simplefin_ignored.json"));
    }

    #[test]
    fn should_report_a_source_that_could_not_be_saved() {
        let layout = linked_layout("unsaved");
        let root = layout.sources.parent().unwrap().to_path_buf();
        // A read-only directory: sources.yaml still reads, the atomic write can't.
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();

        let added = auto_link_new_accounts(&layout, &set_with(&[("ACT-2", "Savings", "5")]));

        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(added.unwrap_err().contains("Chase Bank Savings"));
    }
}
