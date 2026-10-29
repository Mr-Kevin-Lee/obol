//! Pure logic behind `obol link simplefin` (issue 37): which Bridge
//! accounts are still unlinked, a best-guess asset/liability + type for
//! each (SimpleFIN carries neither), and the `SourceConfig` to persist.
//! All I/O (prompts, network, Keychain) stays in the CLI.

use std::collections::HashSet;

use serde_json::json;

use crate::simplefin::AccountSet;
use crate::simplefin_provider::sanitize;
use crate::{Category, SourceConfig};

/// A Bridge account the user hasn't linked to a source yet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkableAccount {
    pub account_id: String,
    pub name: String,
    /// The connection's name, which the protocol says includes the
    /// institution (e.g. "Chase Bank").
    pub institution: String,
    /// Shown so the user can recognize the account (and sanity-check the
    /// liability sign). Kept as SimpleFIN's numeric string.
    pub balance: String,
}

/// Accounts in `set` that no existing `simplefin` source points at.
pub fn linkable_accounts(set: &AccountSet, existing: &[SourceConfig]) -> Vec<LinkableAccount> {
    let linked: HashSet<&str> = existing
        .iter()
        .filter(|s| s.provider == "simplefin")
        .filter_map(|s| s.provider_config.get("simplefin_account_id")?.as_str())
        .collect();

    set.accounts
        .iter()
        .filter(|a| !linked.contains(a.id.as_str()))
        .map(|a| LinkableAccount {
            account_id: a.id.clone(),
            // Server text that is printed and saved as the source's
            // institution, so strip control characters.
            name: sanitize(&a.name),
            institution: set
                .connections
                .iter()
                .find(|c| c.conn_id == a.conn_id)
                .map_or_else(|| "SimpleFIN".to_string(), |c| sanitize(&c.name)),
            balance: sanitize(&a.balance),
        })
        .collect()
}

/// Best guess from the account name only; the user confirms or overrides.
/// Cash-account words (checking, savings, money market, debit) are tested
/// before "card"/"credit" so a "Credit Union Checking" or a "Visa Debit
/// Card" isn't read as a credit card.
pub fn guess_account(name: &str) -> (Category, &'static str) {
    let lower = name.to_lowercase();
    let has = |words: &[&str]| words.iter().any(|w| lower.contains(w));
    // "ira" as a substring would match unrelated words, so match it whole.
    let is_ira = lower
        .split(|c: char| !c.is_alphanumeric())
        .any(|t| t == "ira");

    if has(&["checking", "debit"]) {
        (Category::Asset, "checking")
    } else if has(&["saving"]) {
        (Category::Asset, "savings")
    } else if has(&["money market", "money_market"]) {
        (Category::Asset, "money_market")
    } else if has(&["card", "credit"]) {
        (Category::Liability, "credit_card")
    } else if has(&["loan", "mortgage"]) {
        (Category::Liability, "loan")
    } else if is_ira || has(&["401", "brokerage", "invest", "retire"]) {
        (Category::Asset, "investment")
    } else {
        (Category::Asset, "other")
    }
}

/// A typed account type as lowercase snake_case ("Money Market" ->
/// "money_market"), because downstream code matches exact strings such as
/// "checking" and "money_market". Empty if nothing usable was typed.
pub fn normalize_account_type(typed: &str) -> String {
    slugify(typed)
}

/// The suggested account type for `category`. The name guess only applies
/// when it agrees with the chosen category; otherwise (an overdrawn
/// "Checking" taken as a liability, a "Card" taken as an asset) a type
/// like `checking` or `credit_card` would contradict it, so use `other`.
pub fn default_account_type(name: &str, category: Category) -> &'static str {
    let (guessed_category, guessed_type) = guess_account(name);
    if guessed_category == category {
        guessed_type
    } else {
        "other"
    }
}

/// The suggested category: a positive balance is an asset and a negative
/// one a liability (what a card owes shows as negative). A zero or
/// unreadable balance can't say, so the name guess decides. The user can
/// still override either way.
pub fn default_category(balance: &str, name: &str) -> Category {
    match balance.trim().parse::<f64>() {
        Ok(b) if b > 0.0 => Category::Asset,
        Ok(b) if b < 0.0 => Category::Liability,
        _ => guess_account(name).0,
    }
}

/// Builds the source to hand to `add_source` (which assigns the salt). The
/// id is a slug of institution + account name, suffixed `_2`, `_3`... if
/// it collides with an existing source id.
pub fn new_source(
    account: &LinkableAccount,
    category: Category,
    account_type: &str,
    existing: &[SourceConfig],
) -> SourceConfig {
    let slug = slugify(&format!("{} {}", account.institution, account.name));
    let base = if slug.is_empty() {
        "simplefin_account".to_string()
    } else {
        slug
    };
    let taken = |id: &str| existing.iter().any(|s| s.id == id);
    let id = if taken(&base) {
        (2..)
            .map(|n| format!("{base}_{n}"))
            .find(|candidate| !taken(candidate))
            .expect("an unbounded range always yields a free id")
    } else {
        base
    };

    SourceConfig {
        id,
        provider: "simplefin".into(),
        category,
        account_type: account_type.into(),
        institution: account.institution.clone(),
        // Replaced by `add_source` (D15).
        account_salt: String::new(),
        provider_config: json!({ "simplefin_account_id": account.account_id }),
    }
}

fn slugify(text: &str) -> String {
    text.to_lowercase()
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|part| !part.is_empty())
        .collect::<Vec<_>>()
        .join("_")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simplefin::{SimpleFinAccount, SimpleFinConnection};
    use serde_json::json;

    fn set_with(accounts: &[(&str, &str, &str, &str)]) -> AccountSet {
        AccountSet {
            errlist: vec![],
            connections: vec![
                SimpleFinConnection {
                    conn_id: "c1".into(),
                    name: "Chase Bank".into(),
                },
                SimpleFinConnection {
                    conn_id: "c2".into(),
                    name: "Amex".into(),
                },
            ],
            accounts: accounts
                .iter()
                .map(|(id, name, conn, balance)| SimpleFinAccount {
                    id: id.to_string(),
                    name: name.to_string(),
                    conn_id: conn.to_string(),
                    balance: balance.to_string(),
                })
                .collect(),
        }
    }

    fn source(id: &str, provider: &str, account_id: Option<&str>) -> SourceConfig {
        SourceConfig {
            id: id.into(),
            provider: provider.into(),
            category: Category::Asset,
            account_type: "checking".into(),
            institution: "Chase".into(),
            account_salt: "salt".into(),
            provider_config: match account_id {
                Some(a) => json!({ "simplefin_account_id": a }),
                None => json!({}),
            },
        }
    }

    fn account(institution: &str, name: &str) -> LinkableAccount {
        LinkableAccount {
            account_id: "acc_1".into(),
            name: name.into(),
            institution: institution.into(),
            balance: "1.00".into(),
        }
    }

    #[test]
    fn linkable_accounts_use_the_connection_name_as_institution() {
        let set = set_with(&[("a1", "Checking", "c1", "10.00")]);
        assert_eq!(linkable_accounts(&set, &[])[0].institution, "Chase Bank");
    }

    #[test]
    fn linkable_accounts_strip_control_characters_from_server_text() {
        let mut set = set_with(&[("a1", "Check\u{1b}[2Jing\n", "c1", "10.00\u{7}")]);
        set.connections[0].name = "Chase\u{1b}[31m Bank".into();

        let linkable = &linkable_accounts(&set, &[])[0];

        let all = format!(
            "{}{}{}",
            linkable.name, linkable.institution, linkable.balance
        );
        assert!(!all.chars().any(|c| c.is_control()), "got: {all:?}");
    }

    #[test]
    fn linkable_accounts_keep_the_balance_string() {
        let set = set_with(&[("a1", "Checking", "c1", "10.00")]);
        assert_eq!(linkable_accounts(&set, &[])[0].balance, "10.00");
    }

    #[test]
    fn linkable_accounts_fall_back_when_the_connection_is_missing() {
        let set = set_with(&[("a1", "Checking", "c_unknown", "10.00")]);
        assert_eq!(linkable_accounts(&set, &[])[0].institution, "SimpleFIN");
    }

    #[test]
    fn linkable_accounts_skip_accounts_already_linked_to_a_simplefin_source() {
        let set = set_with(&[
            ("a1", "Checking", "c1", "10.00"),
            ("a2", "Card", "c2", "-5.00"),
        ]);
        let existing = [source("chase_checking", "simplefin", Some("a1"))];
        let linkable = linkable_accounts(&set, &existing);
        assert_eq!(linkable.len(), 1);
        assert_eq!(linkable[0].account_id, "a2");
    }

    #[test]
    fn linkable_accounts_ignore_other_providers_sources() {
        let set = set_with(&[("a1", "Checking", "c1", "10.00")]);
        let existing = [source("plaid_checking", "plaid", Some("a1"))];
        assert_eq!(linkable_accounts(&set, &existing).len(), 1);
    }

    #[test]
    fn linkable_accounts_ignore_simplefin_sources_without_an_account_id() {
        let set = set_with(&[("a1", "Checking", "c1", "10.00")]);
        let existing = [source("broken", "simplefin", None)];
        assert_eq!(linkable_accounts(&set, &existing).len(), 1);
    }

    #[test]
    fn guess_treats_a_card_as_a_credit_card_liability() {
        assert_eq!(
            guess_account("Sapphire Preferred Card"),
            (Category::Liability, "credit_card")
        );
    }

    #[test]
    fn guess_treats_credit_in_the_name_as_a_credit_card_liability() {
        assert_eq!(
            guess_account("Platinum CREDIT"),
            (Category::Liability, "credit_card")
        );
    }

    #[test]
    fn guess_recognizes_a_money_market_before_the_word_credit() {
        assert_eq!(
            guess_account("Credit Union Money Market"),
            (Category::Asset, "money_market")
        );
    }

    #[test]
    fn guess_treats_a_debit_card_as_checking_not_a_credit_card() {
        assert_eq!(
            guess_account("Visa Debit Card"),
            (Category::Asset, "checking")
        );
    }

    #[test]
    fn guess_treats_a_loan_as_a_liability() {
        assert_eq!(guess_account("Auto Loan"), (Category::Liability, "loan"));
    }

    #[test]
    fn guess_treats_a_mortgage_as_a_loan_liability() {
        assert_eq!(
            guess_account("Home Mortgage"),
            (Category::Liability, "loan")
        );
    }

    #[test]
    fn guess_recognizes_checking() {
        assert_eq!(
            guess_account("Total Checking"),
            (Category::Asset, "checking")
        );
    }

    #[test]
    fn guess_recognizes_savings() {
        assert_eq!(
            guess_account("High Yield Savings"),
            (Category::Asset, "savings")
        );
    }

    #[test]
    fn guess_recognizes_investment_accounts() {
        assert_eq!(guess_account("Roth IRA"), (Category::Asset, "investment"));
    }

    #[test]
    fn guess_recognizes_a_401k() {
        assert_eq!(
            guess_account("401(k) Plan"),
            (Category::Asset, "investment")
        );
    }

    #[test]
    fn guess_recognizes_a_brokerage() {
        assert_eq!(guess_account("Brokerage"), (Category::Asset, "investment"));
    }

    #[test]
    fn guess_defaults_unknown_names_to_an_other_asset() {
        assert_eq!(guess_account("Mystery Account"), (Category::Asset, "other"));
    }

    #[test]
    fn a_typed_type_is_lowercased() {
        assert_eq!(normalize_account_type("Checking"), "checking");
    }

    #[test]
    fn spaces_in_a_typed_type_become_underscores() {
        assert_eq!(normalize_account_type("Money Market"), "money_market");
    }

    #[test]
    fn punctuation_and_surrounding_space_in_a_typed_type_collapse() {
        assert_eq!(
            normalize_account_type("  High-Yield  Savings! "),
            "high_yield_savings"
        );
    }

    #[test]
    fn an_already_normal_type_is_unchanged() {
        assert_eq!(normalize_account_type("credit_card"), "credit_card");
    }

    #[test]
    fn a_typed_type_with_nothing_usable_is_empty() {
        assert_eq!(normalize_account_type("***"), "");
    }

    #[test]
    fn the_type_guess_is_used_when_it_agrees_with_the_category() {
        assert_eq!(
            default_account_type("Total Checking", Category::Asset),
            "checking"
        );
    }

    #[test]
    fn a_liability_card_keeps_the_credit_card_type() {
        assert_eq!(
            default_account_type("Visa Card", Category::Liability),
            "credit_card"
        );
    }

    #[test]
    fn an_overdrawn_checking_account_taken_as_a_liability_is_other() {
        assert_eq!(
            default_account_type("Total Checking", Category::Liability),
            "other"
        );
    }

    #[test]
    fn a_card_taken_as_an_asset_is_other() {
        assert_eq!(default_account_type("Visa Card", Category::Asset), "other");
    }

    #[test]
    fn an_unrecognized_name_is_other() {
        assert_eq!(
            default_account_type("Mystery Account", Category::Liability),
            "other"
        );
    }

    #[test]
    fn a_positive_balance_defaults_to_asset_even_if_the_name_says_card() {
        assert_eq!(default_category("12.50", "Visa Card"), Category::Asset);
    }

    #[test]
    fn a_negative_balance_defaults_to_liability_even_if_the_name_says_checking() {
        assert_eq!(
            default_category("-5.00", "Total Checking"),
            Category::Liability
        );
    }

    #[test]
    fn a_zero_balance_falls_back_to_the_name_guess() {
        assert_eq!(default_category("0.00", "Visa Card"), Category::Liability);
    }

    #[test]
    fn an_unreadable_balance_falls_back_to_the_name_guess() {
        assert_eq!(default_category("n/a", "Visa Card"), Category::Liability);
    }

    #[test]
    fn surrounding_whitespace_in_the_balance_is_ignored() {
        assert_eq!(
            default_category("  -5.00 ", "Total Checking"),
            Category::Liability
        );
    }

    #[test]
    fn new_source_slugs_institution_and_name_into_the_id() {
        let s = new_source(
            &account("Chase Bank", "Total Checking"),
            Category::Asset,
            "checking",
            &[],
        );
        assert_eq!(s.id, "chase_bank_total_checking");
    }

    #[test]
    fn new_source_slug_drops_punctuation_and_collapses_separators() {
        let s = new_source(
            &account("Chase Bank (Login #1)", "Sav.  2"),
            Category::Asset,
            "savings",
            &[],
        );
        assert_eq!(s.id, "chase_bank_login_1_sav_2");
    }

    #[test]
    fn new_source_suffixes_the_id_on_collision() {
        let existing = [source("chase_bank_total_checking", "plaid", None)];
        let s = new_source(
            &account("Chase Bank", "Total Checking"),
            Category::Asset,
            "checking",
            &existing,
        );
        assert_eq!(s.id, "chase_bank_total_checking_2");
    }

    #[test]
    fn new_source_keeps_counting_past_an_existing_suffix() {
        let existing = [
            source("chase_bank_total_checking", "plaid", None),
            source("chase_bank_total_checking_2", "plaid", None),
        ];
        let s = new_source(
            &account("Chase Bank", "Total Checking"),
            Category::Asset,
            "checking",
            &existing,
        );
        assert_eq!(s.id, "chase_bank_total_checking_3");
    }

    #[test]
    fn new_source_falls_back_when_the_slug_would_be_empty() {
        let s = new_source(&account("***", "???"), Category::Asset, "other", &[]);
        assert_eq!(s.id, "simplefin_account");
    }

    #[test]
    fn new_source_points_at_the_simplefin_account() {
        let s = new_source(
            &account("Chase Bank", "Total Checking"),
            Category::Asset,
            "checking",
            &[],
        );
        assert_eq!(s.provider, "simplefin");
        assert_eq!(
            s.provider_config,
            json!({ "simplefin_account_id": "acc_1" })
        );
    }

    #[test]
    fn new_source_carries_category_type_and_institution() {
        let s = new_source(
            &account("Chase Bank", "Card"),
            Category::Liability,
            "credit_card",
            &[],
        );
        assert_eq!(s.category, Category::Liability);
        assert_eq!(s.account_type, "credit_card");
        assert_eq!(s.institution, "Chase Bank");
    }
}
