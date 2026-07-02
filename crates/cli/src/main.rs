mod mode;

use std::path::PathBuf;

use clap::{Parser, Subcommand};

use mode::{determine_mode, Mode, RequestedCommand};

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

    // Real screens/fetching land in later tasks (23–26) — this is
    // deliberately still just the dispatch skeleton (task 22's scope).
    match determine_mode(sources.is_empty(), requested) {
        Mode::FirstRunSources => {
            println!("No sources configured yet — Sources screen goes here (task 24).");
        }
        Mode::Dashboard => {
            println!("Dashboard goes here (task 23).");
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
