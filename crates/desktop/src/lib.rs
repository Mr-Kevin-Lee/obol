//! The Tauri desktop frontend (spec §6.1, §15, §16 v0.3, D43) — the
//! GUI that replaces the `ratatui` TUI, sitting on the same `obol-core`
//! as the CLI does. No domain logic lives here: this crate assembles
//! core's output into a shape the webview can render and nothing more,
//! which is what keeps D43's "no logic rewrite" honest.

pub mod dashboard;
pub mod view;

#[cfg(test)]
mod test_support;

use dashboard::{RefreshResult, StorageLayout};
use view::DashboardView;

fn storage_layout() -> Result<StorageLayout, String> {
    let root = dashboard::resolve_storage_root(
        std::env::var_os("OBOL_STORAGE_DIR"),
        std::env::var_os("HOME"),
    )?;
    Ok(StorageLayout::under(&root))
}

// Both commands are `async` so Tauri runs them off the main thread —
// a synchronous command would block the window while it reads disk.

/// What the window shows on launch: disk only, no providers, no network.
#[tauri::command]
async fn get_dashboard() -> Result<Option<DashboardView>, String> {
    dashboard::load_dashboard(&storage_layout()?)
}

/// Fetches every configured source fresh (D43, superseding D19).
#[tauri::command]
async fn refresh() -> Result<RefreshResult, String> {
    let layout = storage_layout()?;
    let registry = dashboard::production_registry(&layout);
    dashboard::refresh_dashboard(&layout, &registry).await
}

pub fn run() {
    tauri::Builder::default()
        .invoke_handler(tauri::generate_handler![get_dashboard, refresh])
        .run(tauri::generate_context!())
        .expect("error while running obol desktop app");
}
