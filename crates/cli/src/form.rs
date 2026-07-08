//! Add/edit source form validation (spec §10.1) — kept as pure,
//! testable logic separate from the terminal prompting that gathers
//! these values (`sources_screen.rs`), per Phase H's test-tier split
//! ("non-rendering logic backing those screens still gets unit tests").
//!
//! Scoped to the three *generic* providers for now — `manual_entry`,
//! `webdriver`, and `statement_import`. Plaid sources are added through
//! a separate hosted-auth flow (§10.1, task 25), not this form.

use obol_core::{Category, SourceConfig};

#[derive(Debug, Clone, Default)]
pub struct SourceFormInput {
    pub id: String,
    pub provider: String,
    pub category: String,
    pub account_type: String,
    pub institution: String,
    /// Only meaningful when `provider == "webdriver"` (spec §10.1's
    /// `WebDriverProviderConfig`).
    pub webdriver_login_url: Option<String>,
    /// Only meaningful when `provider == "statement_import"` (spec
    /// §6.3, D28) — the directory `StatementImportProvider` scans for
    /// PDF statements. Required for that provider.
    pub watch_dir: Option<String>,
    /// Only meaningful when `provider == "statement_import"` —
    /// disambiguates which account a multi-account statement's balance
    /// belongs to (a last-4 digit string, or an employer/plan-name
    /// substring for Fidelity NetBenefits). Optional even for that
    /// provider: statements covering a single account don't need it.
    pub account_hint: Option<String>,
}

/// Validates a form input, returning every problem found (not just the
/// first) so a form can show them all at once. `editing_id` is `Some`
/// when this is an edit of an existing source (so its own id doesn't
/// trip the duplicate-id check against itself); `None` for an add.
pub fn validate(
    input: &SourceFormInput,
    existing_ids: &[String],
    editing_id: Option<&str>,
) -> Vec<String> {
    let mut errors = Vec::new();

    if input.id.trim().is_empty() {
        errors.push("id must not be empty".to_string());
    } else if editing_id != Some(input.id.as_str()) && existing_ids.iter().any(|id| id == &input.id)
    {
        errors.push(format!("a source with id '{}' already exists", input.id));
    }

    if !matches!(
        input.provider.as_str(),
        "manual_entry" | "webdriver" | "statement_import"
    ) {
        errors.push(format!(
            "unknown provider '{}' — must be 'manual_entry', 'webdriver', or \
             'statement_import' (Plaid connects through a separate flow)",
            input.provider
        ));
    }

    if !matches!(input.category.as_str(), "asset" | "liability") {
        errors.push("category must be 'asset' or 'liability'".to_string());
    }

    if input.account_type.trim().is_empty() {
        errors.push("type must not be empty".to_string());
    }

    if input.institution.trim().is_empty() {
        errors.push("institution must not be empty".to_string());
    }

    if input.provider == "webdriver" {
        let valid_url = input
            .webdriver_login_url
            .as_deref()
            .is_some_and(|url| url.starts_with("http://") || url.starts_with("https://"));
        if !valid_url {
            errors.push(
                "webdriver sources need a login_url starting with http:// or https://".to_string(),
            );
        }
    }

    if input.provider == "statement_import" {
        let valid_watch_dir = input.watch_dir.as_deref().is_some_and(|d| !d.trim().is_empty());
        if !valid_watch_dir {
            errors.push(
                "statement_import sources need a watch_dir (the directory to scan for PDF \
                 statements)"
                    .to_string(),
            );
        }
    }

    errors
}

/// Builds the `SourceConfig` to hand to `add_source`/`edit_source`.
/// Only call this once `validate` has returned no errors — it assumes
/// `category`/`provider` are already one of the valid values checked
/// there. `account_salt` is left blank; both `add_source` (always) and
/// `edit_source` (preserving the original) are responsible for the real
/// value (D15) — this form never generates or edits that field itself.
pub fn to_source_config(input: &SourceFormInput) -> SourceConfig {
    let category = match input.category.as_str() {
        "liability" => Category::Liability,
        _ => Category::Asset,
    };
    let provider_config = if input.provider == "webdriver" {
        serde_json::json!({ "login_url": input.webdriver_login_url })
    } else if input.provider == "statement_import" {
        let mut config = serde_json::json!({ "watch_dir": input.watch_dir });
        if let Some(hint) = input.account_hint.as_deref().filter(|h| !h.trim().is_empty()) {
            config["account_hint"] = serde_json::json!(hint);
        }
        config
    } else {
        serde_json::json!({})
    };

    SourceConfig {
        id: input.id.clone(),
        provider: input.provider.clone(),
        category,
        account_type: input.account_type.clone(),
        institution: input.institution.clone(),
        account_salt: String::new(),
        provider_config,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_manual_entry() -> SourceFormInput {
        SourceFormInput {
            id: "apple_card".into(),
            provider: "manual_entry".into(),
            category: "liability".into(),
            account_type: "credit_card".into(),
            institution: "Goldman Sachs".into(),
            webdriver_login_url: None,
            watch_dir: None,
            account_hint: None,
        }
    }

    fn valid_webdriver() -> SourceFormInput {
        SourceFormInput {
            id: "student_loan_navient".into(),
            provider: "webdriver".into(),
            category: "liability".into(),
            account_type: "student_loan".into(),
            institution: "Navient".into(),
            webdriver_login_url: Some("https://navient.com/login".into()),
            watch_dir: None,
            account_hint: None,
        }
    }

    fn valid_statement_import() -> SourceFormInput {
        SourceFormInput {
            id: "chase_checking_statements".into(),
            provider: "statement_import".into(),
            category: "asset".into(),
            account_type: "checking".into(),
            institution: "Chase".into(),
            webdriver_login_url: None,
            watch_dir: Some("/Users/kevin/Statements/Chase".into()),
            account_hint: None,
        }
    }

    #[test]
    fn a_valid_manual_entry_form_has_no_errors() {
        assert!(validate(&valid_manual_entry(), &[], None).is_empty());
    }

    #[test]
    fn a_valid_webdriver_form_has_no_errors() {
        assert!(validate(&valid_webdriver(), &[], None).is_empty());
    }

    #[test]
    fn a_valid_statement_import_form_has_no_errors() {
        assert!(validate(&valid_statement_import(), &[], None).is_empty());
    }

    #[test]
    fn empty_id_is_an_error() {
        let mut input = valid_manual_entry();
        input.id = "  ".into();
        assert!(validate(&input, &[], None)
            .iter()
            .any(|e| e.contains("id must not be empty")));
    }

    #[test]
    fn duplicate_id_is_an_error_on_add() {
        let input = valid_manual_entry();
        let existing = vec!["apple_card".to_string()];
        assert!(validate(&input, &existing, None)
            .iter()
            .any(|e| e.contains("already exists")));
    }

    #[test]
    fn editing_a_source_does_not_trip_the_duplicate_check_against_itself() {
        let input = valid_manual_entry();
        let existing = vec!["apple_card".to_string()];
        assert!(validate(&input, &existing, Some("apple_card")).is_empty());
    }

    #[test]
    fn unknown_provider_is_an_error() {
        let mut input = valid_manual_entry();
        input.provider = "plaid".into();
        assert!(validate(&input, &[], None)
            .iter()
            .any(|e| e.contains("unknown provider")));
    }

    #[test]
    fn invalid_category_is_an_error() {
        let mut input = valid_manual_entry();
        input.category = "checking".into();
        assert!(validate(&input, &[], None)
            .iter()
            .any(|e| e.contains("category must be")));
    }

    #[test]
    fn empty_type_is_an_error() {
        let mut input = valid_manual_entry();
        input.account_type = "".into();
        assert!(validate(&input, &[], None)
            .iter()
            .any(|e| e.contains("type must not be empty")));
    }

    #[test]
    fn empty_institution_is_an_error() {
        let mut input = valid_manual_entry();
        input.institution = "".into();
        assert!(validate(&input, &[], None)
            .iter()
            .any(|e| e.contains("institution must not be empty")));
    }

    #[test]
    fn webdriver_without_a_login_url_is_an_error() {
        let mut input = valid_webdriver();
        input.webdriver_login_url = None;
        assert!(validate(&input, &[], None)
            .iter()
            .any(|e| e.contains("login_url")));
    }

    #[test]
    fn webdriver_with_a_non_http_login_url_is_an_error() {
        let mut input = valid_webdriver();
        input.webdriver_login_url = Some("not-a-url".into());
        assert!(validate(&input, &[], None)
            .iter()
            .any(|e| e.contains("login_url")));
    }

    #[test]
    fn statement_import_without_a_watch_dir_is_an_error() {
        let mut input = valid_statement_import();
        input.watch_dir = None;
        assert!(validate(&input, &[], None)
            .iter()
            .any(|e| e.contains("watch_dir")));
    }

    #[test]
    fn statement_import_with_a_blank_watch_dir_is_an_error() {
        let mut input = valid_statement_import();
        input.watch_dir = Some("   ".into());
        assert!(validate(&input, &[], None)
            .iter()
            .any(|e| e.contains("watch_dir")));
    }

    #[test]
    fn statement_import_without_an_account_hint_has_no_errors() {
        // account_hint is optional — only needed to disambiguate a
        // multi-account statement.
        assert!(validate(&valid_statement_import(), &[], None).is_empty());
    }

    #[test]
    fn multiple_problems_are_all_reported_at_once() {
        let input = SourceFormInput {
            id: "".into(),
            provider: "bogus".into(),
            category: "bogus".into(),
            account_type: "".into(),
            institution: "".into(),
            webdriver_login_url: None,
            watch_dir: None,
            account_hint: None,
        };
        let errors = validate(&input, &[], None);
        assert_eq!(errors.len(), 5);
    }

    #[test]
    fn to_source_config_maps_liability_category_correctly() {
        let config = to_source_config(&valid_manual_entry());
        assert_eq!(config.category, Category::Liability);
        assert_eq!(config.id, "apple_card");
        assert_eq!(config.provider, "manual_entry");
    }

    #[test]
    fn to_source_config_embeds_the_webdriver_login_url_in_provider_config() {
        let config = to_source_config(&valid_webdriver());
        assert_eq!(
            config
                .provider_config
                .get("login_url")
                .and_then(|v| v.as_str()),
            Some("https://navient.com/login")
        );
    }

    #[test]
    fn to_source_config_embeds_the_watch_dir_in_provider_config() {
        let config = to_source_config(&valid_statement_import());
        assert_eq!(
            config
                .provider_config
                .get("watch_dir")
                .and_then(|v| v.as_str()),
            Some("/Users/kevin/Statements/Chase")
        );
        assert!(config.provider_config.get("account_hint").is_none());
    }

    #[test]
    fn to_source_config_embeds_the_account_hint_when_given() {
        let mut input = valid_statement_import();
        input.account_hint = Some("6789".into());
        let config = to_source_config(&input);
        assert_eq!(
            config
                .provider_config
                .get("account_hint")
                .and_then(|v| v.as_str()),
            Some("6789")
        );
    }
}
