//! Statement dropbox provider (spec §6.3, decision D28) — a third
//! `Provider` implementation alongside Plaid and WebDriver. Parses the
//! current balance out of a PDF statement the user has dropped into a
//! per-source directory (`provider_config.watch_dir`), rather than
//! calling a live API or driving a real browser session.
//!
//! Module layout:
//! - [`processed_files`] — which statement files have already been
//!   parsed, and each source's last-known balance.
//! - [`pdf_text`] — plain-text PDF extraction.
//! - [`parser`] — the `StatementParser` trait + institution dispatch.
//! - `chase` — the reference `StatementParser` implementation.
//! - `vanguard`, `fidelity` — sibling `StatementParser` implementations,
//!   added the same way `chase` was: one new module + one match arm in
//!   `parser::parser_for`.
//! - [`discovery`] — auto-discovers new `statement_import` sources from
//!   a fixed `<root>/<Institution>/<Account>` directory convention
//!   (spec D29), rather than requiring each one to be added by hand
//!   through the Sources screen.
//!
//! `StatementImportProvider` below ties these into the `Provider` trait.

mod chase;
mod discovery;
mod fidelity;
mod parser;
mod pdf_text;
mod processed_files;
mod vanguard;

pub use discovery::discover_statement_sources;
pub use parser::{parser_for, ExpectedAccount, ParseError, ParsedStatement, StatementParser};
pub use pdf_text::{extract_text, ExtractError};
pub use processed_files::ProcessedFilesLedger;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::SystemTime;

use async_trait::async_trait;
use sha2::{Digest, Sha256};

use crate::pii::hash_account_number;
use crate::statement_import_storage::{load_or_init_processed_files, save_processed_files};
use crate::{Account, AccountStatus, Asset, Category, Credentials, Liability};
use crate::{Provider, ProviderError, SourceConfig};

/// Scans `provider_config.watch_dir` for the newest not-yet-processed
/// PDF each call, rather than watching the filesystem in the
/// background — obol's CLI is already synchronous (load → fetch → save
/// → render, one invocation, §6.2), so there's nothing for a watcher to
/// add. No `notify`-crate dependency.
pub struct StatementImportProvider {
    ledger_path: PathBuf,
}

impl StatementImportProvider {
    pub fn new(ledger_path: PathBuf) -> Self {
        Self { ledger_path }
    }
}

#[async_trait]
impl Provider for StatementImportProvider {
    async fn fetch(
        &self,
        source: &SourceConfig,
        _credentials: Option<&Credentials>,
    ) -> Result<Vec<Box<dyn Account>>, ProviderError> {
        // No credentials needed — this is local file reading, same
        // shape as the planned ManualEntryProvider.
        let watch_dir = source
            .provider_config
            .get("watch_dir")
            .and_then(|v| v.as_str())
            .ok_or_else(|| {
                ProviderError::Other(format!(
                    "source {} is missing provider_config.watch_dir",
                    source.id
                ))
            })?;
        let watch_dir = Path::new(watch_dir);

        if !watch_dir.is_dir() {
            return Err(ProviderError::Other(format!(
                "source {}'s watch_dir '{}' does not exist or is not a directory",
                source.id,
                watch_dir.display()
            )));
        }

        let expected_hint = source
            .provider_config
            .get("account_hint")
            .and_then(|v| v.as_str())
            .map(String::from);

        let mut ledger = load_or_init_processed_files(&self.ledger_path).map_err(|e| {
            ProviderError::Other(format!("processed-files ledger could not be read: {e}"))
        })?;

        let candidate = newest_unprocessed_pdf(watch_dir, &source.id, &ledger).map_err(|e| {
            ProviderError::Other(format!(
                "could not scan watch_dir '{}': {e}",
                watch_dir.display()
            ))
        })?;

        let (balance, _as_of_date, account_identifier) = match candidate {
            Some((path, filename, content_hash)) => {
                let parser = parser_for(&source.institution).ok_or_else(|| {
                    ProviderError::Other(format!(
                        "no statement parser registered for institution '{}'",
                        source.institution
                    ))
                })?;

                let text = extract_text(&path).map_err(|e| {
                    ProviderError::Other(format!(
                        "failed to extract text from {}: {e}",
                        path.display()
                    ))
                })?;

                let expected = ExpectedAccount {
                    account_type: source.account_type.clone(),
                    account_hint: expected_hint,
                };

                let parsed = parser.parse(&text, &expected).map_err(|e| {
                    ProviderError::Other(format!(
                        "could not parse statement '{filename}' for source {}: {e:?}",
                        source.id
                    ))
                })?;

                ledger.mark_processed(
                    &source.id,
                    &filename,
                    &content_hash,
                    parsed.balance,
                    &parsed.as_of_date,
                    &parsed.account_identifier,
                );
                save_processed_files(&self.ledger_path, &ledger).map_err(|e| {
                    ProviderError::Other(format!("could not save processed-files ledger: {e}"))
                })?;

                (parsed.balance, parsed.as_of_date, parsed.account_identifier)
            }
            None => {
                // Nothing new since last run — a dropbox only gets a
                // new statement monthly, so this is the common case,
                // not an error. Report the same balance/identifier the
                // last successful parse produced, so account_key stays
                // stable (D15) and the dashboard doesn't go blank
                // between statements.
                let (balance, as_of_date, account_identifier) =
                    ledger.last_known(&source.id).ok_or_else(|| {
                        ProviderError::Other(format!(
                            "source {}'s watch_dir has no statements yet — drop a PDF \
                             statement in '{}'",
                            source.id,
                            watch_dir.display()
                        ))
                    })?;
                (balance, as_of_date.to_string(), account_identifier.to_string())
            }
        };

        let account_key = hash_account_number(&account_identifier, &source.account_salt);

        let account: Box<dyn Account> = match source.category {
            Category::Asset => Box::new(Asset {
                account_key,
                institution: source.institution.clone(),
                r#type: source.account_type.clone(),
                balance: Some(balance),
                status: AccountStatus::Ok,
            }),
            Category::Liability => Box::new(Liability {
                account_key,
                institution: source.institution.clone(),
                r#type: source.account_type.clone(),
                balance: Some(balance),
                status: AccountStatus::Ok,
            }),
            Category::Unknown => {
                return Err(ProviderError::Other(format!(
                    "source {} has an unrecognized category — can't tell if this is an \
                     asset or a liability",
                    source.id
                )))
            }
        };

        Ok(vec![account])
    }
}

/// Lists `watch_dir`, filters to `*.pdf` (case-insensitive), skips
/// anything the ledger already has recorded for this source (by
/// content hash, not just filename — a same-named re-download with
/// different bytes is treated as new), and returns the most recently
/// modified remaining file, if any. A dropbox can accumulate older
/// statements over time; only the newest matters for a current-balance
/// snapshot (this isn't a transaction ledger).
fn newest_unprocessed_pdf(
    watch_dir: &Path,
    source_id: &str,
    ledger: &ProcessedFilesLedger,
) -> std::io::Result<Option<(PathBuf, String, String)>> {
    let mut candidates: Vec<(PathBuf, String, String, SystemTime)> = Vec::new();

    for entry in fs::read_dir(watch_dir)? {
        let entry = entry?;
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
        let Some(filename) = path.file_name().and_then(|n| n.to_str()).map(String::from) else {
            continue;
        };

        let bytes = fs::read(&path)?;
        let content_hash = sha256_hex(&bytes);

        if ledger.is_processed(source_id, &filename, &content_hash) {
            continue;
        }

        let mtime = entry.metadata()?.modified()?;
        candidates.push((path, filename, content_hash, mtime));
    }

    candidates.sort_by_key(|(_, _, _, mtime)| *mtime);
    Ok(candidates
        .pop()
        .map(|(path, filename, hash, _)| (path, filename, hash)))
}

fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::fs;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "obol-statement-import-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn ledger_path(dir: &Path) -> PathBuf {
        dir.join("processed_statements.json")
    }

    fn fixture_bytes() -> Vec<u8> {
        fs::read(
            PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("tests/fixtures/chase_statement_sample.pdf"),
        )
        .unwrap()
    }

    fn source(watch_dir: &Path, account_hint: Option<&str>) -> SourceConfig {
        let mut config = json!({ "watch_dir": watch_dir.to_str().unwrap() });
        if let Some(account_hint) = account_hint {
            config["account_hint"] = json!(account_hint);
        }
        SourceConfig {
            id: "chase_checking".into(),
            provider: "statement_import".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            institution: "Chase".into(),
            account_salt: "test-salt".into(),
            provider_config: config,
        }
    }

    #[tokio::test]
    async fn first_run_processes_the_one_statement_present() {
        let dir = temp_dir("first-run");
        fs::write(dir.join("statement.pdf"), fixture_bytes()).unwrap();

        let provider = StatementImportProvider::new(ledger_path(&dir));
        let accounts = provider.fetch(&source(&dir, None), None).await.unwrap();

        assert_eq!(accounts.len(), 1);
        assert_eq!(accounts[0].balance(), Some(1234.56));
        assert_eq!(accounts[0].institution(), "Chase");

        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_second_fetch_with_no_new_file_returns_the_same_last_known_balance() {
        let dir = temp_dir("no-new-file");
        fs::write(dir.join("statement.pdf"), fixture_bytes()).unwrap();

        let provider = StatementImportProvider::new(ledger_path(&dir));
        let first = provider.fetch(&source(&dir, None), None).await.unwrap();
        let second = provider.fetch(&source(&dir, None), None).await.unwrap();

        assert_eq!(first[0].balance(), second[0].balance());
        // account_key must stay identical too — this is D15's stability
        // guarantee, not just a matching balance figure.
        assert_eq!(first[0].account_key(), second[0].account_key());

        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn missing_watch_dir_config_is_a_provider_error() {
        let dir = temp_dir("missing-config");
        let provider = StatementImportProvider::new(ledger_path(&dir));
        let mut bad_source = source(&dir, None);
        bad_source.provider_config = json!({});

        let err = provider.fetch(&bad_source, None).await.unwrap_err();
        assert!(!err.is_transient());

        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn nonexistent_watch_dir_is_a_provider_error() {
        let dir = temp_dir("nonexistent");
        let provider = StatementImportProvider::new(ledger_path(&dir));
        let missing_dir = dir.join("does-not-exist");

        let err = provider
            .fetch(&source(&missing_dir, None), None)
            .await
            .unwrap_err();
        assert!(!err.is_transient());

        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn unrecognized_institution_is_a_provider_error() {
        let dir = temp_dir("unknown-institution");
        fs::write(dir.join("statement.pdf"), fixture_bytes()).unwrap();

        let provider = StatementImportProvider::new(ledger_path(&dir));
        let mut bad_source = source(&dir, None);
        bad_source.institution = "SomeBankNobodyHasWrittenAParserFor".into();

        let err = provider.fetch(&bad_source, None).await.unwrap_err();
        assert!(!err.is_transient());

        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn empty_watch_dir_with_no_history_is_a_provider_error() {
        let dir = temp_dir("empty-no-history");
        let provider = StatementImportProvider::new(ledger_path(&dir));

        let err = provider.fetch(&source(&dir, None), None).await.unwrap_err();
        assert!(!err.is_transient());

        fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn a_genuinely_new_file_added_between_runs_gets_processed() {
        let dir = temp_dir("new-file-between-runs");
        fs::write(dir.join("statement.pdf"), fixture_bytes()).unwrap();

        let provider = StatementImportProvider::new(ledger_path(&dir));
        let first = provider.fetch(&source(&dir, None), None).await.unwrap();
        assert_eq!(first[0].balance(), Some(1234.56));

        // A second, differently-named statement file (content doesn't
        // matter for this test beyond "it's a new, unprocessed file" —
        // reusing the same fixture bytes is fine since the ledger keys
        // on filename+hash together, and this filename hasn't been seen
        // before for this source).
        fs::write(dir.join("statement-2.pdf"), fixture_bytes()).unwrap();

        let second = provider.fetch(&source(&dir, None), None).await.unwrap();
        assert_eq!(second[0].balance(), Some(1234.56));

        fs::remove_dir_all(&dir).ok();
    }
}
