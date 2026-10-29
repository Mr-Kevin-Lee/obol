//! Edit-source form validation (spec §10.1) — kept as pure, testable
//! logic separate from the terminal prompting that gathers these values
//! (`sources_screen.rs`), per Phase H's test-tier split ("non-rendering
//! logic backing those screens still gets unit tests").
//!
//! Accounts are added by `obol link simplefin`, so this form only edits an
//! existing source's id, category, type and institution; its provider and
//! `provider_config` are carried over untouched.

use obol_core::{Category, SourceConfig};

#[derive(Debug, Clone)]
pub struct SourceFormInput {
    pub id: String,
    pub category: String,
    pub account_type: String,
    pub institution: String,
}

/// Validates a form input, returning every problem found (not just the
/// first) so a form can show them all at once. `editing_id` is the
/// source being edited, so its own id doesn't trip the duplicate-id check
/// against itself.
pub fn validate(input: &SourceFormInput, existing_ids: &[String], editing_id: &str) -> Vec<String> {
    let mut errors = Vec::new();

    if input.id.trim().is_empty() {
        errors.push("id must not be empty".to_string());
    } else if input.id != editing_id && existing_ids.iter().any(|id| id == &input.id) {
        errors.push(format!("a source with id '{}' already exists", input.id));
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

    errors
}

/// Builds the `SourceConfig` to hand to `edit_source`. Only call this once
/// `validate` has returned no errors. `provider` and `provider_config`
/// come from `original`; `account_salt` is left blank because
/// `edit_source` preserves the original (D15) — this form never edits it.
pub fn to_source_config(input: &SourceFormInput, original: &SourceConfig) -> SourceConfig {
    let category = match input.category.as_str() {
        "liability" => Category::Liability,
        _ => Category::Asset,
    };

    SourceConfig {
        id: input.id.clone(),
        provider: original.provider.clone(),
        category,
        account_type: input.account_type.clone(),
        institution: input.institution.clone(),
        account_salt: String::new(),
        provider_config: original.provider_config.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_input() -> SourceFormInput {
        SourceFormInput {
            id: "apple_card".into(),
            category: "liability".into(),
            account_type: "credit_card".into(),
            institution: "Apple Card".into(),
        }
    }

    fn original_source() -> SourceConfig {
        SourceConfig {
            id: "apple_card".into(),
            provider: "simplefin".into(),
            category: Category::Asset,
            account_type: "other".into(),
            institution: "Apple Card (Updated Monthly)".into(),
            account_salt: "salt-from-add-time".into(),
            provider_config: serde_json::json!({ "simplefin_account_id": "ACT-1" }),
        }
    }

    #[test]
    fn a_valid_form_has_no_errors() {
        assert!(validate(&valid_input(), &[], "apple_card").is_empty());
    }

    #[test]
    fn empty_id_is_an_error() {
        let mut input = valid_input();
        input.id = "  ".into();
        assert!(validate(&input, &[], "apple_card")
            .iter()
            .any(|e| e.contains("id must not be empty")));
    }

    #[test]
    fn an_id_taken_by_another_source_is_an_error() {
        let mut input = valid_input();
        input.id = "amex".into();
        let existing = vec!["apple_card".to_string(), "amex".to_string()];
        assert!(validate(&input, &existing, "apple_card")
            .iter()
            .any(|e| e.contains("already exists")));
    }

    #[test]
    fn keeping_the_same_id_does_not_trip_the_duplicate_check_against_itself() {
        let existing = vec!["apple_card".to_string()];
        assert!(validate(&valid_input(), &existing, "apple_card").is_empty());
    }

    #[test]
    fn invalid_category_is_an_error() {
        let mut input = valid_input();
        input.category = "checking".into();
        assert!(validate(&input, &[], "apple_card")
            .iter()
            .any(|e| e.contains("category must be")));
    }

    #[test]
    fn empty_type_is_an_error() {
        let mut input = valid_input();
        input.account_type = "".into();
        assert!(validate(&input, &[], "apple_card")
            .iter()
            .any(|e| e.contains("type must not be empty")));
    }

    #[test]
    fn empty_institution_is_an_error() {
        let mut input = valid_input();
        input.institution = "".into();
        assert!(validate(&input, &[], "apple_card")
            .iter()
            .any(|e| e.contains("institution must not be empty")));
    }

    #[test]
    fn multiple_problems_are_all_reported_at_once() {
        let input = SourceFormInput {
            id: "".into(),
            category: "bogus".into(),
            account_type: "".into(),
            institution: "".into(),
        };
        assert_eq!(validate(&input, &[], "apple_card").len(), 4);
    }

    #[test]
    fn to_source_config_maps_liability_category_correctly() {
        let config = to_source_config(&valid_input(), &original_source());
        assert_eq!(config.category, Category::Liability);
    }

    #[test]
    fn to_source_config_takes_the_edited_fields() {
        let config = to_source_config(&valid_input(), &original_source());
        assert_eq!(
            (
                config.id.as_str(),
                config.account_type.as_str(),
                config.institution.as_str()
            ),
            ("apple_card", "credit_card", "Apple Card")
        );
    }

    #[test]
    fn to_source_config_carries_over_the_provider() {
        let config = to_source_config(&valid_input(), &original_source());
        assert_eq!(config.provider, "simplefin");
    }

    #[test]
    fn to_source_config_carries_over_the_provider_config() {
        let config = to_source_config(&valid_input(), &original_source());
        assert_eq!(
            config.provider_config,
            serde_json::json!({ "simplefin_account_id": "ACT-1" })
        );
    }
}
