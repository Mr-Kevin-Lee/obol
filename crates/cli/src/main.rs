mod dashboard;
mod form;
mod mode;
mod sources_screen;

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
    let snapshots_dir = storage_dir.join("snapshots");
    let sources = match obol_core::load_or_init(&sources_path) {
        Ok(sources) => sources,
        Err(err) => {
            // §9.1: a malformed sources.yaml blocks the whole run with a
            // clear message rather than falling back to an empty list.
            eprintln!("sources.yaml could not be loaded: {err}");
            std::process::exit(1);
        }
    };

    // Headless snapshot wiring is still a placeholder below — Dashboard
    // (task 23) and Sources (task 24) are wired to the real engine.
    match determine_mode(sources.is_empty(), requested) {
        Mode::FirstRunSources | Mode::Sources => {
            if let Err(err) = sources_screen::run(&sources_path, &item_usage_path, &snapshots_dir) {
                eprintln!("sources screen failed: {err}");
                std::process::exit(1);
            }
        }
        Mode::Dashboard => {
            let mut registry = obol_core::provider_registry();
            maybe_register_plaid(&mut registry);
            let credential_source = NoInteractiveProvidersYet;

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
