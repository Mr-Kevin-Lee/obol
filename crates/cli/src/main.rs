mod dashboard;
mod form;
mod mode;
mod sources_screen;

use std::io::Write;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use mode::{determine_mode, Mode, RequestedCommand};
use obol_core::{CredentialSource, Credentials, SourceConfig};

/// No interactive `Provider` (webdriver, manual entry) is registered
/// yet — this is a placeholder that always declines, to be replaced
/// with a real terminal-prompt implementation once one exists. Plaid
/// sources never reach this at all (§8, resolved from Keychain
/// internally by the engine).
struct NoInteractiveProvidersYet;

impl CredentialSource for NoInteractiveProvidersYet {
    fn provide(&self, _source: &SourceConfig) -> Option<Credentials> {
        None
    }
}

/// Dev/testing-only wiring (see spec.md D24's addendum) — registers a
/// real `PlaidProvider` if `PLAID_CLIENT_ID`/`PLAID_SECRET` are set in
/// the environment, mirroring the same env-var convention used
/// elsewhere for these credentials (D20). No-op (registry stays empty
/// for "plaid") if they're not set. `PLAID_ENVIRONMENT=production`
/// opts into real Production; anything else (including unset) stays on
/// Sandbox, matching the project's "test against Sandbox first"
/// discipline (§7).
fn maybe_register_plaid(
    registry: &mut std::collections::HashMap<&'static str, obol_core::ProviderFactory>,
) {
    let (Ok(client_id), Ok(secret)) = (
        std::env::var("PLAID_CLIENT_ID"),
        std::env::var("PLAID_SECRET"),
    ) else {
        return;
    };
    let environment = if std::env::var("PLAID_ENVIRONMENT").as_deref() == Ok("production") {
        obol_core::PlaidEnvironment::Production
    } else {
        obol_core::PlaidEnvironment::Sandbox
    };

    registry.insert(
        "plaid",
        Box::new(move || {
            let client = obol_core::PlaidClient::new(obol_core::PlaidConfig {
                client_id: client_id.clone(),
                secret: secrecy::Secret::new(secret.clone()),
                environment: environment.clone(),
            });
            Box::new(obol_core::PlaidProvider::new(client)) as Box<dyn obol_core::Provider>
        }),
    );
}

/// Registers the statement-import provider (spec §6.3, D28) —
/// unconditional, unlike `maybe_register_plaid`, since this provider
/// reads local files rather than calling an external API and so has no
/// credentials to gate on. Needs only a path for its processed-files
/// ledger, which lives alongside `item_usage.json` under the storage
/// directory.
fn register_statement_import(
    registry: &mut std::collections::HashMap<&'static str, obol_core::ProviderFactory>,
    processed_statements_path: &std::path::Path,
) {
    let ledger_path = processed_statements_path.to_path_buf();
    registry.insert(
        "statement_import",
        Box::new(move || {
            Box::new(obol_core::StatementImportProvider::new(ledger_path.clone()))
                as Box<dyn obol_core::Provider>
        }),
    );
}

#[derive(Parser)]
#[command(
    name = "obol",
    version,
    about = "A locally-run financial health dashboard"
)]
struct Cli {
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Fetch and save a snapshot, no TUI rendering — for scheduled/headless runs.
    Snapshot,
    /// Jump straight to the Sources screen.
    Sources,
}

/// `~/Library/Application Support/obol/` (spec §4, §11.1) — outside any
/// cloud-synced folder, fixed rather than configurable in v1 (D17).
fn storage_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    PathBuf::from(home).join("Library/Application Support/obol")
}

/// `~/Statements/` (spec §6.3 addendum, D29) — the root of the
/// `<Institution>/<Account>` convention `discover_statement_sources`
/// scans. Fixed rather than configurable in v1, same as `storage_dir`
/// (D17).
fn statements_root() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    PathBuf::from(home).join("Statements")
}

/// Loads emergency-fund thresholds (spec §13.1 Type A, D36) from
/// `rules.yaml`, prompting once for the target monthly-expense figure
/// if it's still unset — the one value with no sensible default (the
/// red/yellow/green band thresholds already have spec-given defaults).
/// Fails soft on a malformed `rules.yaml` (falls back to defaults + a
/// warning) rather than exiting the process, unlike `sources.yaml`'s
/// fail-hard treatment (§9.1) — a broken rules file degrades one
/// dashboard panel, not the whole run. **Only ever called from the
/// interactive Dashboard screen**, never from a future headless `obol
/// snapshot` path, since that can't block on stdin.
fn load_emergency_fund_thresholds_interactive(
    path: &std::path::Path,
) -> obol_core::EmergencyFundThresholds {
    let mut thresholds =
        obol_core::load_or_init_emergency_fund_thresholds(path).unwrap_or_else(|err| {
            eprintln!("warning: rules.yaml could not be read ({err}) — using defaults");
            obol_core::EmergencyFundThresholds::default()
        });

    if thresholds.target_monthly_expenses <= 0.0 {
        if let Some(value) = prompt_for_target_monthly_expenses() {
            thresholds.target_monthly_expenses = value;
            if let Err(err) = obol_core::save_emergency_fund_thresholds(path, &thresholds) {
                eprintln!("warning: could not save target monthly expenses: {err}");
            }
        }
    }

    thresholds
}

/// Plain stdin/stdout prompt — must run *before* `dashboard::run()`
/// enters raw/alternate-screen mode, so this stays here rather than
/// inside `ratatui` rendering code. One attempt: blank input skips
/// (asked again next run), an invalid entry also skips rather than
/// looping — a light first-run nicety, not a validation gauntlet.
fn prompt_for_target_monthly_expenses() -> Option<f64> {
    print!("Emergency fund tracking: enter your target monthly expenses ($, blank to skip): ");
    std::io::stdout().flush().ok()?;
    let mut input = String::new();
    std::io::stdin().read_line(&mut input).ok()?;
    let trimmed = input.trim();
    if trimmed.is_empty() {
        return None;
    }
    match trimmed.parse::<f64>() {
        Ok(value) if value > 0.0 => Some(value),
        _ => {
            eprintln!("Not a valid positive number — skipping for now.");
            None
        }
    }
}

/// Wires `core::engine`'s audit events (§4, task 26) to a local file —
/// core only ever emits `tracing` events, never decides where they go
/// (§6.1: no UI/presentation concerns in core); this is that decision,
/// made once per interface. `0600`, same protection as every other file
/// in the storage directory (§4). Silently does nothing if the file
/// can't be opened (e.g. read-only filesystem) — a missing audit log is
/// a degraded-but-survivable condition, not one worth crashing over.
fn init_audit_log(storage_dir: &std::path::Path) {
    let _ = std::fs::create_dir_all(storage_dir);
    let log_path = storage_dir.join("audit.log");
    let Ok(file) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&log_path)
    else {
        return;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&log_path, std::fs::Permissions::from_mode(0o600));
    }
    let _ = tracing_subscriber::fmt()
        .with_writer(std::sync::Mutex::new(file))
        .with_ansi(false)
        .try_init();
}

#[tokio::main]
async fn main() {
    let cli = Cli::parse();
    let requested = match cli.command {
        None => RequestedCommand::Default,
        Some(Commands::Snapshot) => RequestedCommand::Snapshot,
        Some(Commands::Sources) => RequestedCommand::Sources,
    };

    let storage_dir = storage_dir();
    init_audit_log(&storage_dir);
    let sources_path = storage_dir.join("sources.yaml");
    let item_usage_path = storage_dir.join("item_usage.json");
    let processed_statements_path = storage_dir.join("processed_statements.json");
    let snapshots_dir = storage_dir.join("snapshots");
    let rules_path = storage_dir.join("rules.yaml");
    let mut sources = match obol_core::load_or_init(&sources_path) {
        Ok(sources) => sources,
        Err(err) => {
            // §9.1: a malformed sources.yaml blocks the whole run with a
            // clear message rather than falling back to an empty list.
            // Deliberately checked before auto-discovery below — a
            // broken config should never be written to while it's
            // already broken.
            eprintln!("sources.yaml could not be loaded: {err}");
            std::process::exit(1);
        }
    };

    // Statement auto-discovery (spec D29): runs once per process, here
    // at startup — matching this CLI's one-shot synchronous invocation
    // model (no background watcher). A directory added while obol is
    // already running won't be picked up until the next invocation.
    // Pushing newly-added sources into the in-memory `sources` directly
    // (rather than re-reading the file) keeps `determine_mode` below
    // accurate without a third disk read.
    for new_source in obol_core::discover_statement_sources(&statements_root(), &sources) {
        match obol_core::add_source(&sources_path, new_source.clone()) {
            Ok(()) => sources.push(new_source),
            Err(err) => eprintln!("warning: could not add auto-discovered source: {err}"),
        }
    }

    match determine_mode(sources.is_empty(), requested) {
        Mode::FirstRunSources | Mode::Sources => {
            run_screen_loop(
                Screen::Sources,
                &sources_path,
                &item_usage_path,
                &processed_statements_path,
                &snapshots_dir,
                &rules_path,
            )
            .await;
        }
        Mode::Dashboard => {
            run_screen_loop(
                Screen::Dashboard,
                &sources_path,
                &item_usage_path,
                &processed_statements_path,
                &snapshots_dir,
                &rules_path,
            )
            .await;
        }
        Mode::SnapshotHeadless => {
            println!(
                "Headless snapshot goes here — pending a registered provider (ManualEntryProvider/PlaidProvider)."
            );
        }
        Mode::NothingToSnapshot => {
            println!("No sources configured — nothing to snapshot.");
        }
    }
}

enum Screen {
    Dashboard,
    Sources,
}

/// Bounces between the Dashboard and Sources screens on their `s`/`v`
/// signals (dashboard.rs's `DashboardAction`/sources_screen.rs's
/// `SourcesAction`) rather than requiring a full quit-and-rerun to
/// switch between viewing and managing sources — either screen's own
/// quit key exits this loop, and the process, entirely. Re-entering
/// Dashboard always re-fetches: if you just came from editing sources,
/// you want that reflected, not a stale snapshot from before the loop
/// started.
async fn run_screen_loop(
    mut screen: Screen,
    sources_path: &std::path::Path,
    item_usage_path: &std::path::Path,
    processed_statements_path: &std::path::Path,
    snapshots_dir: &std::path::Path,
    rules_path: &std::path::Path,
) {
    loop {
        screen = match screen {
            Screen::Sources => {
                match sources_screen::run(sources_path, item_usage_path, snapshots_dir).await {
                    Ok(sources_screen::SourcesAction::Quit) => return,
                    Ok(sources_screen::SourcesAction::GoToDashboard) => Screen::Dashboard,
                    Err(err) => {
                        eprintln!("sources screen failed: {err}");
                        std::process::exit(1);
                    }
                }
            }
            Screen::Dashboard => {
                let sources = match obol_core::load_or_init(sources_path) {
                    Ok(sources) => sources,
                    Err(err) => {
                        eprintln!("sources.yaml could not be loaded: {err}");
                        std::process::exit(1);
                    }
                };
                // Captured before run_and_save's own save happens, so
                // it's unambiguous regardless of whether that save
                // succeeds — "the most recent snapshot from before this
                // run," not "whatever's now newest on disk."
                let previous = obol_core::load_recent_snapshots(snapshots_dir, 1)
                    .unwrap_or_default()
                    .into_iter()
                    .next();

                let mut registry = obol_core::provider_registry();
                maybe_register_plaid(&mut registry);
                register_statement_import(&mut registry, processed_statements_path);
                let credential_source = NoInteractiveProvidersYet;

                let result =
                    obol_core::run_and_save(&sources, &registry, &credential_source, snapshots_dir)
                        .await;
                if let Some(err) = &result.save_error {
                    // §9.1: best-effort persistence — a save failure
                    // never blocks rendering what was just fetched.
                    eprintln!("warning: this run's data was not saved to history: {err}");
                }

                // Reloaded fresh every entry (same treatment as
                // `sources` above) — a hand-edit made while on the
                // Sources screen, or a value just entered via the
                // first-run prompt below, is picked up without
                // restarting.
                let emergency_fund_thresholds =
                    load_emergency_fund_thresholds_interactive(rules_path);

                match dashboard::run(&result.snapshot, previous.as_ref(), &emergency_fund_thresholds) {
                    Ok(dashboard::DashboardAction::Quit) => return,
                    Ok(dashboard::DashboardAction::GoToSources) => Screen::Sources,
                    Err(err) => {
                        eprintln!("dashboard rendering failed: {err}");
                        std::process::exit(1);
                    }
                }
            }
        };
    }
}
