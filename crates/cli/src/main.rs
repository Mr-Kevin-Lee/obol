mod dashboard;
mod form;
mod link;
mod mode;
mod monthly_spend_screen;
mod recommendations_screen;
mod sources_screen;

use std::io::Write;
use std::path::PathBuf;

use clap::{Parser, Subcommand};

use mode::{determine_mode, Mode, RequestedCommand};
use obol_core::{CredentialSource, Credentials, SourceConfig};

/// No interactive `Provider` (manual entry) is registered yet — this is
/// a placeholder that always declines, to be replaced with a real
/// terminal-prompt implementation once one exists. SimpleFIN sources
/// never reach this at all (the engine resolves their Access URL).
struct NoInteractiveProvidersYet;

impl CredentialSource for NoInteractiveProvidersYet {
    fn provide(&self, _source: &SourceConfig) -> Option<Credentials> {
        None
    }
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
    /// Link a data provider's accounts as sources.
    Link {
        #[command(subcommand)]
        provider: LinkProvider,
    },
}

#[derive(Subcommand)]
enum LinkProvider {
    /// Claim a SimpleFIN Setup Token and add its accounts.
    Simplefin,
}

/// `~/Library/Application Support/obol/` (spec §4, §11.1) — outside any
/// cloud-synced folder, fixed rather than configurable in v1 (D17).
fn storage_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    PathBuf::from(home).join("Library/Application Support/obol")
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
        Some(Commands::Link {
            provider: LinkProvider::Simplefin,
        }) => {
            link::run_simplefin(&storage_dir().join("sources.yaml")).await;
            return;
        }
    };

    let storage_dir = storage_dir();
    init_audit_log(&storage_dir);
    let sources_path = storage_dir.join("sources.yaml");
    let snapshots_dir = storage_dir.join("snapshots");
    let rules_path = storage_dir.join("rules.yaml");
    let sources = match obol_core::load_or_init(&sources_path) {
        Ok(sources) => sources,
        Err(err) => {
            // §9.1: a malformed sources.yaml blocks the whole run with a
            // clear message rather than falling back to an empty list.
            eprintln!("sources.yaml could not be loaded: {err}");
            std::process::exit(1);
        }
    };

    match determine_mode(sources.is_empty(), requested) {
        Mode::FirstRunSources | Mode::Sources => {
            run_screen_loop(Screen::Sources, &sources_path, &snapshots_dir, &rules_path).await;
        }
        Mode::Dashboard => {
            run_screen_loop(
                Screen::Dashboard,
                &sources_path,
                &snapshots_dir,
                &rules_path,
            )
            .await;
        }
        Mode::SnapshotHeadless => {
            println!("Headless snapshot is not implemented yet.");
        }
        Mode::NothingToSnapshot => {
            println!("No sources configured — nothing to snapshot.");
        }
    }
}

enum Screen {
    Dashboard,
    Sources,
    Recommendations,
    MonthlySpendChart,
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
    snapshots_dir: &std::path::Path,
    rules_path: &std::path::Path,
) {
    loop {
        screen = match screen {
            Screen::Sources => match sources_screen::run(sources_path, snapshots_dir) {
                Ok(sources_screen::SourcesAction::Quit) => return,
                Ok(sources_screen::SourcesAction::GoToDashboard) => Screen::Dashboard,
                Err(err) => {
                    eprintln!("sources screen failed: {err}");
                    std::process::exit(1);
                }
            },
            Screen::Recommendations => match recommendations_screen::run(rules_path) {
                Ok(recommendations_screen::RecommendationsAction::Quit) => return,
                Ok(recommendations_screen::RecommendationsAction::GoToDashboard) => {
                    Screen::Dashboard
                }
                Err(err) => {
                    eprintln!("recommendations screen failed: {err}");
                    std::process::exit(1);
                }
            },
            Screen::MonthlySpendChart => {
                match monthly_spend_screen::run(rules_path, snapshots_dir) {
                    Ok(monthly_spend_screen::MonthlySpendAction::Quit) => return,
                    Ok(monthly_spend_screen::MonthlySpendAction::GoToDashboard) => {
                        Screen::Dashboard
                    }
                    Err(err) => {
                        eprintln!("monthly spend chart screen failed: {err}");
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
                obol_core::register_simplefin(
                    &mut registry,
                    sources_path.with_file_name("simplefin_usage.json"),
                );
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
                let checklist_statuses = obol_core::load_or_init_checklist_statuses(rules_path)
                    .unwrap_or_else(|err| {
                        eprintln!(
                            "warning: rules.yaml could not be read ({err}) — using an empty checklist"
                        );
                        obol_core::ChecklistStatuses::default()
                    });
                let monthly_spend_thresholds = obol_core::load_or_init_monthly_spend_thresholds(
                    rules_path,
                )
                .unwrap_or_else(|err| {
                    eprintln!("warning: rules.yaml could not be read ({err}) — using defaults");
                    obol_core::MonthlySpendThresholds::default()
                });
                let debt_payoff_config = obol_core::load_or_init_debt_payoff_config(rules_path)
                    .unwrap_or_else(|err| {
                        eprintln!("warning: rules.yaml could not be read ({err}) — using defaults");
                        obol_core::DebtPayoffConfig::default()
                    });

                match dashboard::run(
                    &result.snapshot,
                    previous.as_ref(),
                    &emergency_fund_thresholds,
                    &checklist_statuses,
                    &monthly_spend_thresholds,
                    &debt_payoff_config,
                ) {
                    Ok(dashboard::DashboardAction::Quit) => return,
                    Ok(dashboard::DashboardAction::GoToSources) => Screen::Sources,
                    Ok(dashboard::DashboardAction::GoToRecommendations) => Screen::Recommendations,
                    Ok(dashboard::DashboardAction::GoToMonthlySpendChart) => {
                        Screen::MonthlySpendChart
                    }
                    Err(err) => {
                        eprintln!("dashboard rendering failed: {err}");
                        std::process::exit(1);
                    }
                }
            }
        };
    }
}
