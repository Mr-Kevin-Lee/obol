//! The Tauri desktop frontend (spec §6.1, §15, §16 v0.3, D43) — the
//! GUI that replaces the `ratatui` TUI, sitting on the same `obol-core`
//! as the CLI does. No domain logic lives here: this crate assembles
//! core's output into a shape the webview can render and nothing more,
//! which is what keeps D43's "no logic rewrite" honest.

pub mod auto_link;
pub mod budget;
pub mod connect;
pub mod dashboard;
pub mod labels;
pub mod notes;
pub mod simplefin;
pub mod sync;
pub mod view;

#[cfg(test)]
mod test_support;

use std::sync::Mutex;

use budget::{ImportResult, SpendingPlanView, TransactionsView};
use connect::{ConnectAccount, ConnectPreview, ConnectStatus, LinkResult};
use dashboard::{RefreshResult, StorageLayout};
use obol_core::{parse_iso_date, Date, RecurringSeries, SeriesKind, TransactionRef, YearMonth};
use simplefin::SimpleFin;
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

/// Serializes every read-modify-write the app makes: `merchant_rules.yaml`,
/// `plan.yaml`, `sources.yaml` with the SimpleFIN ignore list, and the
/// transaction files with their sync record. Two
/// quick edits would otherwise race: both read, both write, and the first
/// edit is lost. In-process only — §9.1's file lock has no production
/// callers yet. The CLI can write `sources.yaml` too, but not while you're
/// also linking in the app.
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

fn today() -> Date {
    // UTC, like the posting dates SimpleFIN transactions are stored under.
    time::OffsetDateTime::now_utc().date()
}

/// Fetches every configured source fresh (D43, superseding D19). First the
/// SimpleFIN half: one request for balances and recent transactions, which
/// the snapshot run then reuses, plus linking accounts opened since and
/// fetching any history still missing.
#[tauri::command]
async fn refresh(
    lock: tauri::State<'_, WriteLock>,
    simplefin: tauri::State<'_, SimpleFin>,
) -> Result<RefreshResult, String> {
    let layout = storage_layout()?;
    let access_url = simplefin.access_url().await;
    let outcome = match &access_url {
        Some(url) => {
            let bridge = sync::SharedBridge {
                simplefin: &simplefin,
                access_url: url,
            };
            sync::sync_on_refresh(&layout, &bridge, &lock.0, today()).await?
        }
        None => sync::SyncOutcome::default(),
    };
    let mut result =
        dashboard::refresh_dashboard(&layout, &simplefin.registry(), access_url).await?;
    result.added_sources = outcome.added_sources;
    result.transactions_added = outcome.transactions_added;
    result.sync_warning = outcome.warning;
    Ok(result)
}

/// One more request: the 90 days of SimpleFIN transactions before the
/// earliest day fetched.
#[tauri::command]
async fn load_earlier_transactions(
    lock: tauri::State<'_, WriteLock>,
    simplefin: tauri::State<'_, SimpleFin>,
) -> Result<sync::EarlierResult, String> {
    let layout = storage_layout()?;
    let access_url = simplefin
        .access_url()
        .await
        .ok_or("SimpleFIN isn't connected, so there's no history to load.")?;
    let bridge = sync::SharedBridge {
        simplefin: &simplefin,
        access_url: &access_url,
    };
    sync::load_earlier(&layout, &bridge, &lock.0, today()).await
}

/// Whether the Net worth screen should offer to connect SimpleFIN.
#[tauri::command]
async fn connect_status() -> Result<ConnectStatus, String> {
    connect::connect_status(&storage_layout()?)
}

/// `setup_token: None` reuses the Access URL already in the Keychain.
#[tauri::command]
async fn preview_simplefin(
    simplefin: tauri::State<'_, SimpleFin>,
    setup_token: Option<String>,
) -> Result<ConnectPreview, String> {
    connect::preview(&storage_layout()?, &simplefin, setup_token).await
}

#[tauri::command]
async fn link_simplefin_accounts(
    lock: tauri::State<'_, WriteLock>,
    accounts: Vec<ConnectAccount>,
) -> Result<LinkResult, String> {
    let layout = storage_layout()?;
    locked(&lock, || connect::link_accounts(&layout, &accounts))
}

#[tauri::command]
async fn get_transactions(month: String) -> Result<TransactionsView, String> {
    budget::transactions_view(&storage_layout()?, parse_month(&month)?)
}

/// Names (or, with a blank label, un-names) one account, and returns the
/// refreshed balances screen.
#[tauri::command]
async fn set_account_label(
    lock: tauri::State<'_, WriteLock>,
    source_id: String,
    label: String,
) -> Result<Option<DashboardView>, String> {
    let layout = storage_layout()?;
    locked(&lock, || labels::set_label(&layout, &source_id, &label))?;
    dashboard::load_dashboard(&layout)
}

#[tauri::command]
async fn set_transaction_excluded(
    lock: tauri::State<'_, WriteLock>,
    month: String,
    source_id: String,
    id: String,
    excluded: bool,
) -> Result<TransactionsView, String> {
    let (layout, month) = (storage_layout()?, parse_month(&month)?);
    let transaction = TransactionRef { source_id, id };
    locked(&lock, || {
        budget::set_excluded(&layout, transaction, excluded)
    })?;
    budget::transactions_view(&layout, month)
}

/// Writes (or, with a blank note, clears) the note on one transaction.
#[tauri::command]
async fn set_transaction_note(
    lock: tauri::State<'_, WriteLock>,
    month: String,
    source_id: String,
    id: String,
    note: String,
) -> Result<TransactionsView, String> {
    let (layout, month) = (storage_layout()?, parse_month(&month)?);
    locked(&lock, || notes::set_note(&layout, &source_id, &id, &note))?;
    budget::transactions_view(&layout, month)
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

/// Learns categories from a Simplifi export's text, which the webview reads
/// from the file the user picked. Additive: every existing rule is kept.
#[tauri::command]
async fn import_simplifi(
    lock: tauri::State<'_, WriteLock>,
    contents: String,
) -> Result<ImportResult, String> {
    let layout = storage_layout()?;
    locked(&lock, || budget::import_simplifi(&layout, &contents))
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
    // Without a storage root every command fails with the reason anyway;
    // the request log then has nowhere to go, which only loosens the
    // request budget's guard.
    let usage_log = storage_layout()
        .map(|layout| layout.simplefin_usage)
        .unwrap_or_default();
    tauri::Builder::default()
        .manage(WriteLock(Mutex::new(())))
        .manage(SimpleFin::new(usage_log))
        .invoke_handler(tauri::generate_handler![
            get_dashboard,
            refresh,
            connect_status,
            preview_simplefin,
            link_simplefin_accounts,
            load_earlier_transactions,
            get_transactions,
            set_merchant_category,
            set_account_label,
            set_transaction_excluded,
            set_transaction_note,
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
