//! The Tauri desktop frontend (spec §6.1, §15, §16 v0.3, D43) — the
//! GUI that replaces the `ratatui` TUI, sitting on the same `obol-core`
//! as the CLI does. No domain logic lives here: this crate assembles
//! core's output into a shape the webview can render and nothing more,
//! which is what keeps D43's "no logic rewrite" honest.

pub mod budget;
pub mod dashboard;
pub mod view;

#[cfg(test)]
mod test_support;

use std::sync::Mutex;

use budget::{ImportResult, SpendingPlanView, TransactionsView};
use dashboard::{RefreshResult, StorageLayout};
use obol_core::{parse_iso_date, Date, RecurringSeries, SeriesKind, YearMonth};
use view::DashboardView;

fn storage_layout() -> Result<StorageLayout, String> {
    let root = dashboard::resolve_storage_root(
        std::env::var_os("OBOL_STORAGE_DIR"),
        std::env::var_os("HOME"),
    )?;
    Ok(StorageLayout::under(&root))
}

fn parse_month(text: &str) -> Result<YearMonth, String> {
    YearMonth::parse(text).ok_or_else(|| format!("invalid month {text:?}, expected YYYY-MM"))
}

fn parse_day(text: &str) -> Result<Date, String> {
    parse_iso_date(text).ok_or_else(|| format!("invalid date {text:?}, expected YYYY-MM-DD"))
}

/// Serializes every read-modify-write of `merchant_rules.yaml` and
/// `plan.yaml`. Two quick edits would otherwise race: both read, both
/// write, and the first edit is lost. In-process only — §9.1's file lock
/// has no production callers yet, and no other process writes these files.
struct WriteLock(Mutex<()>);

fn locked<T>(lock: &WriteLock, write: impl FnOnce() -> Result<T, String>) -> Result<T, String> {
    let _guard = lock
        .0
        .lock()
        .map_err(|_| "an earlier edit failed partway; restart the app".to_string())?;
    write()
}

// Every command is `async` so Tauri runs it off the main thread — a
// synchronous command would block the window while it reads disk. Each
// mutation returns the refreshed view, so the screen re-renders from one
// round trip.

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

#[tauri::command]
async fn get_transactions(month: String) -> Result<TransactionsView, String> {
    budget::transactions_view(&storage_layout()?, parse_month(&month)?)
}

#[tauri::command]
async fn set_merchant_category(
    lock: tauri::State<'_, WriteLock>,
    month: String,
    merchant_key: String,
    category: String,
) -> Result<TransactionsView, String> {
    let (layout, month) = (storage_layout()?, parse_month(&month)?);
    locked(&lock, || {
        budget::set_category(&layout, &merchant_key, &category)
    })?;
    budget::transactions_view(&layout, month)
}

/// Additive: re-importing adds only new rows and keeps every existing rule.
#[tauri::command]
async fn import_simplifi(
    lock: tauri::State<'_, WriteLock>,
    path: String,
) -> Result<ImportResult, String> {
    let layout = storage_layout()?;
    locked(&lock, || budget::import_simplifi(&layout, &path))
}

#[tauri::command]
async fn get_spending_plan(month: String, today: String) -> Result<SpendingPlanView, String> {
    budget::spending_plan_view(&storage_layout()?, parse_month(&month)?, parse_day(&today)?)
}

#[tauri::command]
async fn set_category_target(
    lock: tauri::State<'_, WriteLock>,
    month: String,
    today: String,
    category: String,
    target: Option<f64>,
) -> Result<SpendingPlanView, String> {
    let (layout, month, today) = (storage_layout()?, parse_month(&month)?, parse_day(&today)?);
    locked(&lock, || budget::set_target(&layout, &category, target))?;
    budget::spending_plan_view(&layout, month, today)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // one per field the webview sends
async fn confirm_series(
    lock: tauri::State<'_, WriteLock>,
    month: String,
    today: String,
    merchant_key: String,
    kind: SeriesKind,
    expected_amount: f64,
    due_day: u8,
) -> Result<SpendingPlanView, String> {
    let (layout, month, today) = (storage_layout()?, parse_month(&month)?, parse_day(&today)?);
    let series = RecurringSeries {
        merchant_key,
        kind,
        expected_amount,
        due_day,
    };
    locked(&lock, || budget::confirm_series(&layout, series))?;
    budget::spending_plan_view(&layout, month, today)
}

#[tauri::command]
async fn dismiss_series(
    lock: tauri::State<'_, WriteLock>,
    month: String,
    today: String,
    merchant_key: String,
) -> Result<SpendingPlanView, String> {
    let (layout, month, today) = (storage_layout()?, parse_month(&month)?, parse_day(&today)?);
    locked(&lock, || budget::dismiss_series(&layout, &merchant_key))?;
    budget::spending_plan_view(&layout, month, today)
}

#[tauri::command]
async fn remove_series(
    lock: tauri::State<'_, WriteLock>,
    month: String,
    today: String,
    merchant_key: String,
) -> Result<SpendingPlanView, String> {
    let (layout, month, today) = (storage_layout()?, parse_month(&month)?, parse_day(&today)?);
    locked(&lock, || budget::remove_series(&layout, &merchant_key))?;
    budget::spending_plan_view(&layout, month, today)
}

#[tauri::command]
#[allow(clippy::too_many_arguments)] // one per field the webview sends
async fn add_series(
    lock: tauri::State<'_, WriteLock>,
    month: String,
    today: String,
    name: String,
    kind: SeriesKind,
    expected_amount: f64,
    due_day: u8,
) -> Result<SpendingPlanView, String> {
    let (layout, month, today) = (storage_layout()?, parse_month(&month)?, parse_day(&today)?);
    locked(&lock, || {
        budget::add_series(&layout, &name, kind, expected_amount, due_day)
    })?;
    budget::spending_plan_view(&layout, month, today)
}

pub fn run() {
    tauri::Builder::default()
        .manage(WriteLock(Mutex::new(())))
        .invoke_handler(tauri::generate_handler![
            get_dashboard,
            refresh,
            get_transactions,
            set_merchant_category,
            import_simplifi,
            get_spending_plan,
            set_category_target,
            confirm_series,
            dismiss_series,
            remove_series,
            add_series,
        ])
        .run(tauri::generate_context!())
        .expect("error while running obol desktop app");
}
