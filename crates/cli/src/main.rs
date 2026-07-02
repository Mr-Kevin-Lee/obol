mod dashboard;
mod mode;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use mode::{determine_mode, Mode, RequestedCommand};
use obol_core::{CredentialSource, Credentials, SourceConfig};

/// No interactive `Provider` (webdriver, manual entry) is registered
/// yet (tasks 15+) — this is a placeholder that always declines, to be
/// replaced with a real masked-terminal-prompt implementation once one
/// exists. Plaid sources never reach this at all (§8, resolved from
/// Keychain internally by the engine).
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
}

/// `~/Library/Application Support/obol/` (spec §4, §11.1) — outside any
/// cloud-synced folder, fixed rather than configurable in v1 (D17).
fn storage_dir() -> PathBuf {
    let home = std::env::var("HOME").expect("HOME must be set");
    PathBuf::from(home).join("Library/Application Support/obol")
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
    let sources_path = storage_dir.join("sources.yaml");
    let sources = match obol_core::load_or_init(&sources_path) {
        Ok(sources) => sources,
        Err(err) => {
            // §9.1: a malformed sources.yaml blocks the whole run with a
            // clear message rather than falling back to an empty list.
            eprintln!("sources.yaml could not be loaded: {err}");
            std::process::exit(1);
        }
    };

    // Sources screen (task 24) and headless snapshot wiring still just
    // placeholders below — Dashboard (task 23) is the first branch
    // wired to the real engine.
    match determine_mode(sources.is_empty(), requested) {
        Mode::FirstRunSources => {
            println!("No sources configured yet — Sources screen goes here (task 24).");
        }
        Mode::Dashboard => {
            let registry = obol_core::provider_registry();
            let credential_source = NoInteractiveProvidersYet;
            let snapshots_dir = storage_dir.join("snapshots");

            let result =
                obol_core::run_and_save(&sources, &registry, &credential_source, &snapshots_dir)
                    .await;
            if let Some(err) = &result.save_error {
                // §9.1: best-effort persistence — a save failure never
                // blocks rendering what was just fetched.
                eprintln!("warning: this run's data was not saved to history: {err}");
            }
            if let Err(err) = dashboard::run(&result.snapshot) {
                eprintln!("dashboard rendering failed: {err}");
                std::process::exit(1);
            }
        }
        Mode::Sources => {
            println!("Sources screen goes here (task 24).");
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
