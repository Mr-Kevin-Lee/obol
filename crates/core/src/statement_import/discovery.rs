//! Statement auto-discovery (spec §6.3 addendum, D29) — scans a fixed
//! `<root>/<Institution>/<Account>` directory convention and returns a
//! ready-to-`add_source` `SourceConfig` for every leaf directory not
//! already registered as a `statement_import` source's `watch_dir`. One
//! leaf directory = one account = one `watch_dir`, matching
//! `StatementImportProvider`'s existing one-directory-per-source model
//! exactly — this module only ever *creates* sources, it never changes
//! how an existing one is fetched.
//!
//! Every failure mode here (missing root, unrecognized institution, an
//! empty leaf, a statement that fails to parse) is swallowed and
//! printed as a warning rather than propagated as an error — one bad
//! leaf directory must never block discovering everything else, or
//! abort the run. Runs once per process at CLI startup (`main.rs`), not
//! on a background watcher — same "one-shot synchronous invocation, no
//! `notify` dependency" philosophy as the rest of this module (D28).

use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use serde_json::json;

use crate::statement_import::parser::{parser_for, ExpectedAccount};
use crate::statement_import::pdf_text::extract_text;
use crate::{Category, SourceConfig};

/// Scans `statements_root` two levels deep. Returns an empty list
/// (never an error) if the root doesn't exist — that's the common case
/// for anyone not using this feature, not an edge case.
pub fn discover_statement_sources(
    statements_root: &Path,
    existing_sources: &[SourceConfig],
) -> Vec<SourceConfig> {
    if !statements_root.is_dir() {
        return Vec::new();
    }

    let known_watch_dirs: HashSet<&str> = existing_sources
        .iter()
        .filter(|s| s.provider == "statement_import")
        .filter_map(|s| s.provider_config.get("watch_dir").and_then(|v| v.as_str()))
        .collect();

    let Ok(institution_entries) = fs::read_dir(statements_root) else {
        return Vec::new();
    };

    let mut discovered = Vec::new();
    for institution_entry in institution_entries.flatten() {
        let institution_dir = institution_entry.path();
        if !institution_dir.is_dir() {
            continue;
        }
        let Some(institution) = institution_dir.file_name().and_then(|n| n.to_str()) else {
            continue;
        };

        let Ok(account_entries) = fs::read_dir(&institution_dir) else {
            continue;
        };
        for account_entry in account_entries.flatten() {
            let account_dir = account_entry.path();
            if !account_dir.is_dir() {
                continue;
            }
            let Some(account) = account_dir.file_name().and_then(|n| n.to_str()) else {
                continue;
            };
            let Some(watch_dir) = account_dir.to_str() else {
                continue;
            };
            if known_watch_dirs.contains(watch_dir) {
                continue;
            }

            if let Some(config) = discover_one(institution, account, &account_dir, watch_dir) {
                discovered.push(config);
            }
        }
    }

    discovered
}

/// Attempts to build a new source for a single leaf directory. `None`
/// (with a warning printed to stderr) for every way this can fail — a
/// single bad leaf must never abort the rest of the scan.
fn discover_one(
    institution: &str,
    account: &str,
    account_dir: &Path,
    watch_dir: &str,
) -> Option<SourceConfig> {
    let Some(parser) = parser_for(institution) else {
        eprintln!(
            "statement auto-discovery: skipping '{watch_dir}' — no parser registered for \
             institution '{institution}'"
        );
        return None;
    };

    let Some(pdf_path) = newest_pdf(account_dir) else {
        eprintln!("statement auto-discovery: skipping '{watch_dir}' — no PDF statements found yet");
        return None;
    };

    let text = match extract_text(&pdf_path) {
        Ok(text) => text,
        Err(err) => {
            eprintln!(
                "statement auto-discovery: skipping '{watch_dir}' — could not extract text from \
                 {}: {err}",
                pdf_path.display()
            );
            return None;
        }
    };

    let expected = ExpectedAccount {
        account_type: account.to_lowercase(),
        account_hint: None,
    };
    let parsed = match parser.parse(&text, &expected) {
        Ok(parsed) => parsed,
        Err(err) => {
            eprintln!(
                "statement auto-discovery: skipping '{watch_dir}' — could not parse the newest \
                 statement: {err:?}"
            );
            return None;
        }
    };

    // Content stays the primary signal (it's what's actually verified/
    // parseable) — the filename is only ever consulted as a tiebreak
    // when content detection landed on the *uncertain* default
    // (`Asset`, e.g. Chase's heuristic found no explicit liability
    // terminology). It only ever pushes toward `Liability`, never away
    // from a positive content match, and never away from an
    // institution's structurally-guaranteed `Asset` classification in
    // any way that content detection wouldn't already allow.
    let category = if parsed.category == Category::Asset && filename_suggests_liability(&pdf_path)
    {
        Category::Liability
    } else {
        parsed.category
    };

    Some(SourceConfig {
        id: format!("{}_{}", institution.to_lowercase(), account.to_lowercase()),
        provider: "statement_import".into(),
        category,
        account_type: account.to_lowercase(),
        institution: institution.to_string(),
        // Overwritten by `sources::add_source` (D15) — this value is
        // never read back.
        account_salt: String::new(),
        provider_config: json!({ "watch_dir": watch_dir }),
    })
}

/// The most recently modified `*.pdf` directly inside `dir` (not
/// recursive — a deeper nesting level than `<root>/<Institution>/<Account>`
/// is never walked into). Distinct from `mod.rs`'s
/// `newest_unprocessed_pdf`, which filters by `ProcessedFilesLedger` —
/// that doesn't apply here, since the source doesn't exist yet.
fn newest_pdf(dir: &Path) -> Option<PathBuf> {
    let read_dir = fs::read_dir(dir).ok()?;
    let mut candidates: Vec<(PathBuf, SystemTime)> = Vec::new();
    for entry in read_dir.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_pdf = path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf"));
        if !is_pdf {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(mtime) = metadata.modified() else {
            continue;
        };
        candidates.push((path, mtime));
    }
    candidates.sort_by_key(|(_, mtime)| *mtime);
    candidates.pop().map(|(path, _)| path)
}

/// Filename-based liability tiebreak (spec D29 addendum) — a
/// deliberately small, generic keyword list, not institution-specific.
/// Only ever consulted when content detection already landed on the
/// uncertain `Asset` default; see `discover_one`'s doc comment for why.
const LIABILITY_FILENAME_MARKERS: &[&str] =
    &["credit", "card", "visa", "mastercard", "amex", "loan", "mortgage"];

fn filename_suggests_liability(pdf_path: &Path) -> bool {
    let Some(name) = pdf_path.file_name().and_then(|n| n.to_str()) else {
        return false;
    };
    let lower = name.to_lowercase();
    LIABILITY_FILENAME_MARKERS.iter().any(|marker| lower.contains(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Category;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "obol-discovery-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn fixture_bytes() -> Vec<u8> {
        fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/chase_statement_sample.pdf"),
        )
        .unwrap()
    }

    fn existing_source(watch_dir: &str) -> SourceConfig {
        SourceConfig {
            id: "existing".into(),
            provider: "statement_import".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            institution: "Chase".into(),
            account_salt: "test-salt".into(),
            provider_config: json!({ "watch_dir": watch_dir }),
        }
    }

    #[test]
    fn a_missing_statements_root_returns_an_empty_list() {
        let root = temp_dir("missing-root").join("does-not-exist");
        assert!(discover_statement_sources(&root, &[]).is_empty());
    }

    #[test]
    fn discovers_a_new_leaf_directory_with_a_valid_statement() {
        let root = temp_dir("happy-path");
        let leaf = root.join("Chase").join("Checking");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(leaf.join("statement.pdf"), fixture_bytes()).unwrap();

        let discovered = discover_statement_sources(&root, &[]);

        assert_eq!(discovered.len(), 1);
        let source = &discovered[0];
        assert_eq!(source.id, "chase_checking");
        assert_eq!(source.provider, "statement_import");
        assert_eq!(source.institution, "Chase");
        assert_eq!(source.account_type, "checking");
        assert_eq!(source.category, Category::Asset);
        assert_eq!(
            source.provider_config.get("watch_dir").and_then(|v| v.as_str()),
            Some(leaf.to_str().unwrap())
        );

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_already_registered_watch_dir_is_not_rediscovered() {
        let root = temp_dir("already-known");
        let leaf = root.join("Chase").join("Checking");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(leaf.join("statement.pdf"), fixture_bytes()).unwrap();

        let existing = vec![existing_source(leaf.to_str().unwrap())];
        let discovered = discover_statement_sources(&root, &existing);

        assert!(discovered.is_empty());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_unrecognized_institution_directory_is_skipped() {
        let root = temp_dir("unrecognized-institution");
        let leaf = root.join("SomeBankNobodyHasWrittenAParserFor").join("Checking");
        fs::create_dir_all(&leaf).unwrap();

        let discovered = discover_statement_sources(&root, &[]);

        assert!(discovered.is_empty());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn an_empty_leaf_directory_is_skipped() {
        let root = temp_dir("empty-leaf");
        let leaf = root.join("Chase").join("Checking");
        fs::create_dir_all(&leaf).unwrap();

        let discovered = discover_statement_sources(&root, &[]);

        assert!(discovered.is_empty());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_leaf_whose_statement_fails_to_parse_is_skipped() {
        let root = temp_dir("parse-failure");
        // The Chase fixture's text doesn't match Vanguard's layout at
        // all, so this exercises the parse-failure skip path without
        // needing a second real PDF fixture.
        let leaf = root.join("Vanguard").join("Brokerage");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(leaf.join("statement.pdf"), fixture_bytes()).unwrap();

        let discovered = discover_statement_sources(&root, &[]);

        assert!(discovered.is_empty());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn multiple_new_leaves_are_all_discovered_with_distinct_ids() {
        let root = temp_dir("multiple-leaves");
        let checking = root.join("Chase").join("Checking");
        let savings = root.join("Chase").join("Savings");
        fs::create_dir_all(&checking).unwrap();
        fs::create_dir_all(&savings).unwrap();
        fs::write(checking.join("statement.pdf"), fixture_bytes()).unwrap();
        fs::write(savings.join("statement.pdf"), fixture_bytes()).unwrap();

        let mut discovered = discover_statement_sources(&root, &[]);
        discovered.sort_by(|a, b| a.id.cmp(&b.id));

        assert_eq!(discovered.len(), 2);
        assert_eq!(discovered[0].id, "chase_checking");
        assert_eq!(discovered[1].id, "chase_savings");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_stray_file_directly_under_the_root_is_skipped() {
        let root = temp_dir("stray-file");
        fs::write(root.join(".DS_Store"), b"not a directory").unwrap();
        let leaf = root.join("Chase").join("Checking");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(leaf.join("statement.pdf"), fixture_bytes()).unwrap();

        let discovered = discover_statement_sources(&root, &[]);

        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].id, "chase_checking");

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_pdf_nested_a_third_level_deep_is_not_discovered() {
        let root = temp_dir("third-level");
        let leaf = root.join("Chase").join("Checking");
        let nested = leaf.join("Old");
        fs::create_dir_all(&nested).unwrap();
        // No PDF directly inside the leaf — only one level deeper,
        // which discovery never walks into.
        fs::write(nested.join("statement.pdf"), fixture_bytes()).unwrap();

        let discovered = discover_statement_sources(&root, &[]);

        assert!(discovered.is_empty());

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_liability_hinting_filename_overrides_an_asset_default() {
        // The fixture's own content has no liability terminology, so
        // content detection alone would land on Asset — the filename
        // tiebreak is what should flip this one to Liability.
        let root = temp_dir("filename-tiebreak");
        let leaf = root.join("Chase").join("CreditCard");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(leaf.join("chase_credit_card_march.pdf"), fixture_bytes()).unwrap();

        let discovered = discover_statement_sources(&root, &[]);

        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].category, Category::Liability);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn a_plain_filename_does_not_affect_an_asset_default() {
        let root = temp_dir("filename-no-hint");
        let leaf = root.join("Chase").join("Checking");
        fs::create_dir_all(&leaf).unwrap();
        fs::write(leaf.join("march_statement.pdf"), fixture_bytes()).unwrap();

        let discovered = discover_statement_sources(&root, &[]);

        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].category, Category::Asset);

        fs::remove_dir_all(&root).ok();
    }

    #[test]
    fn filename_suggests_liability_matches_known_markers_case_insensitively() {
        assert!(filename_suggests_liability(Path::new("Chase_CreditCard_2026.pdf")));
        assert!(filename_suggests_liability(Path::new("visa_statement.pdf")));
        assert!(!filename_suggests_liability(Path::new("checking_statement.pdf")));
    }
}
