//! Fetching SimpleFIN transactions (#38): Refresh's half, and "load
//! earlier". The decisions — which days, which accounts — are core's
//! (`simplefin_sync`); this sequences the requests and the writes.
//!
//! The write lock is taken only around each file write, never across a
//! request, so editing a category during a Refresh isn't held up by the
//! network.

use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;
use obol_core::{
    earlier_window, format_iso_date, history_windows, load_or_init, load_sync_state, recent_window,
    record_window, save_sync_state, save_transactions, transactions_in, AccountSet, Date, Secret,
    SourceConfig, SyncState, TransactionWindow,
};
use serde::Serialize;

use crate::auto_link;
use crate::dashboard::StorageLayout;
use crate::simplefin::SimpleFin;

/// One request to the Bridge. The app's goes through the shared provider;
/// tests use a fake.
// async_trait marks the boxed-future method `#[must_use]`, which clippy
// flags as doubling the future's own (as on core's `Provider`).
#[allow(clippy::double_must_use)]
#[async_trait]
pub trait Bridge: Sync {
    async fn fetch(&self, window: &TransactionWindow) -> Result<Arc<AccountSet>, String>;
}

/// The app's Bridge: the shared provider, with the remembered Access URL.
pub struct SharedBridge<'a> {
    pub simplefin: &'a SimpleFin,
    pub access_url: &'a Secret<String>,
}

#[async_trait]
impl Bridge for SharedBridge<'_> {
    async fn fetch(&self, window: &TransactionWindow) -> Result<Arc<AccountSet>, String> {
        self.simplefin
            .provider
            .transactions(self.access_url, window)
            .await
            .map_err(|err| err.to_string())
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct SyncOutcome {
    /// `"<institution> <name>"` per account linked on its own.
    pub added_sources: Vec<String>,
    pub transactions_added: usize,
    /// Why older transactions are still missing, when a history request
    /// failed (it's retried on the next Refresh).
    pub warning: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct EarlierResult {
    pub added: usize,
    /// `YYYY-MM-DD`: how far back SimpleFIN history now reaches.
    pub history_from: String,
}

/// Refresh's SimpleFIN half: one request for every balance and the recent
/// transactions (the snapshot run after reuses it), linking accounts opened
/// since, then any history still missing. Sends nothing when no source uses
/// SimpleFIN. A failed recent request is left for the run to report per
/// source.
pub async fn sync_on_refresh(
    layout: &StorageLayout,
    bridge: &impl Bridge,
    lock: &Mutex<()>,
    today: Date,
) -> Result<SyncOutcome, String> {
    let (sources, before) = {
        let _guard = write_lock(lock)?;
        (read_sources(layout)?, read_state(layout)?)
    };
    if !sources.iter().any(|source| source.provider == "simplefin") {
        return Ok(SyncOutcome::default());
    }
    let recent = recent_window(&before, &sources, today);
    let Ok(set) = bridge.fetch(&recent.request(&sources)).await else {
        return Ok(SyncOutcome::default());
    };

    let mut outcome = SyncOutcome::default();
    let (sources, history, mut state) = {
        let _guard = write_lock(lock)?;
        outcome.added_sources = auto_link::auto_link_new_accounts(layout, &set)?;
        let sources = read_sources(layout)?;
        outcome.transactions_added += store(layout, &set, &sources)?;
        // Planned from the state before this request, so accounts just
        // linked, and a gap after a long absence, still get their history.
        let history = history_windows(&before, &sources, today, recent.start);
        let mut state = before;
        record_window(&mut state, &recent, &sources, today);
        write_state(layout, &state)?;
        (sources, history, state)
    };

    for window in history {
        match bridge.fetch(&window.request(&sources)).await {
            Ok(set) => {
                let _guard = write_lock(lock)?;
                outcome.transactions_added += store(layout, &set, &sources)?;
                record_window(&mut state, &window, &sources, today);
                write_state(layout, &state)?;
            }
            Err(err) => {
                outcome.warning = Some(format!(
                    "Older transactions couldn't be fetched yet ({err}). \
                     The next Refresh tries again."
                ));
                break;
            }
        }
    }
    Ok(outcome)
}

/// "Load earlier": the 90 days before the earliest day fetched, one
/// request.
pub async fn load_earlier(
    layout: &StorageLayout,
    bridge: &impl Bridge,
    lock: &Mutex<()>,
    today: Date,
) -> Result<EarlierResult, String> {
    let (sources, mut state) = {
        let _guard = write_lock(lock)?;
        (read_sources(layout)?, read_state(layout)?)
    };
    let window = earlier_window(&state, &sources).ok_or(
        "Nothing has been fetched from SimpleFIN yet, so there's no history to extend. \
         Press Refresh first.",
    )?;
    // The screen already says what failed, so only the reason goes back.
    let set = bridge.fetch(&window.request(&sources)).await?;

    let _guard = write_lock(lock)?;
    let added = store(layout, &set, &sources)?;
    record_window(&mut state, &window, &sources, today);
    write_state(layout, &state)?;
    Ok(EarlierResult {
        added,
        history_from: format_iso_date(window.start),
    })
}

fn write_lock(lock: &Mutex<()>) -> Result<MutexGuard<'_, ()>, String> {
    lock.lock()
        .map_err(|_| "an earlier edit failed partway; restart the app".to_string())
}

/// Like `load_or_init`, minus the init: a refresh never creates the file.
fn read_sources(layout: &StorageLayout) -> Result<Vec<SourceConfig>, String> {
    if !layout.sources.exists() {
        return Ok(Vec::new());
    }
    load_or_init(&layout.sources).map_err(|err| format!("sources.yaml could not be loaded: {err}"))
}

fn read_state(layout: &StorageLayout) -> Result<SyncState, String> {
    load_sync_state(&layout.simplefin_sync).map_err(|err| err.to_string())
}

fn write_state(layout: &StorageLayout, state: &SyncState) -> Result<(), String> {
    save_sync_state(&layout.simplefin_sync, state).map_err(|err| err.to_string())
}

/// Saves `set`'s transactions, returning how many were new.
fn store(
    layout: &StorageLayout,
    set: &AccountSet,
    sources: &[SourceConfig],
) -> Result<usize, String> {
    save_transactions(&layout.transactions, &transactions_in(set, sources))
        .map(|summary| summary.added)
        .map_err(|err| format!("transactions could not be saved: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use obol_core::{
        add_source, load_transactions_range, Category, SimpleFinAccount, SimpleFinConnection,
        SimpleFinTransaction, YearMonth,
    };
    use serde_json::json;
    use std::fs;
    use time::{Duration, Month};

    fn today() -> Date {
        Date::from_calendar_date(2026, Month::November, 4).unwrap()
    }

    fn days_ago(n: i64) -> Date {
        today() - Duration::days(n)
    }

    fn midnight(date: Date) -> i64 {
        date.midnight().assume_utc().unix_timestamp()
    }

    fn temp_layout(name: &str) -> StorageLayout {
        let root = std::env::temp_dir().join(format!(
            "obol-desktop-sync-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        StorageLayout::under(&root)
    }

    fn simplefin_source(id: &str, account_id: &str) -> SourceConfig {
        SourceConfig {
            id: id.into(),
            provider: "simplefin".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            institution: "Chase Bank".into(),
            account_salt: String::new(),
            provider_config: json!({ "simplefin_account_id": account_id }),
        }
    }

    fn layout_with(name: &str, sources: &[SourceConfig]) -> StorageLayout {
        let layout = temp_layout(name);
        for source in sources {
            add_source(&layout.sources, source.clone()).unwrap();
        }
        layout
    }

    fn sf_transaction(id: &str, posted: Date, amount: &str) -> SimpleFinTransaction {
        SimpleFinTransaction {
            id: id.into(),
            posted: midnight(posted) + 43_200,
            amount: amount.into(),
            description: "CORNER CAFE".into(),
            pending: false,
        }
    }

    /// `(account id, name, transactions)`, all at Chase Bank.
    fn set_with(accounts: Vec<(&str, &str, Vec<SimpleFinTransaction>)>) -> Arc<AccountSet> {
        Arc::new(AccountSet {
            errlist: vec![],
            connections: vec![SimpleFinConnection {
                conn_id: "c1".into(),
                name: "Chase Bank".into(),
            }],
            accounts: accounts
                .into_iter()
                .map(|(id, name, transactions)| SimpleFinAccount {
                    id: id.into(),
                    name: name.into(),
                    conn_id: "c1".into(),
                    balance: "100.00".into(),
                    transactions,
                })
                .collect(),
        })
    }

    /// Answers each request in turn from `replies` (repeating the last),
    /// and records every window asked for.
    struct FakeBridge {
        replies: Vec<Result<Arc<AccountSet>, String>>,
        asked: Mutex<Vec<TransactionWindow>>,
    }

    impl FakeBridge {
        fn answering(replies: Vec<Result<Arc<AccountSet>, String>>) -> Self {
            Self {
                replies,
                asked: Mutex::new(Vec::new()),
            }
        }

        fn asked(&self) -> Vec<TransactionWindow> {
            self.asked.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl Bridge for FakeBridge {
        async fn fetch(&self, window: &TransactionWindow) -> Result<Arc<AccountSet>, String> {
            let mut asked = self.asked.lock().unwrap();
            asked.push(window.clone());
            let index = (asked.len() - 1).min(self.replies.len() - 1);
            self.replies[index].clone()
        }
    }

    fn sync_state(layout: &StorageLayout) -> SyncState {
        load_sync_state(&layout.simplefin_sync).unwrap()
    }

    fn stored_ids(layout: &StorageLayout) -> Vec<String> {
        let mut ids: Vec<String> = load_transactions_range(
            &layout.transactions,
            YearMonth::new(2000, 1).unwrap(),
            YearMonth::new(2100, 1).unwrap(),
        )
        .unwrap()
        .into_iter()
        .map(|transaction| transaction.id)
        .collect();
        ids.sort();
        ids
    }

    async fn refresh(layout: &StorageLayout, bridge: &FakeBridge) -> Result<SyncOutcome, String> {
        sync_on_refresh(layout, bridge, &Mutex::new(()), today()).await
    }

    fn window(start: Date, end: Option<Date>, accounts: &[&str]) -> TransactionWindow {
        TransactionWindow {
            start: midnight(start),
            end: end.map(midnight),
            accounts: accounts.iter().map(|a| a.to_string()).collect(),
        }
    }

    // --- sync_on_refresh ---

    #[tokio::test]
    async fn should_send_nothing_when_no_source_uses_simplefin() {
        let layout = temp_layout("no-simplefin");
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![]))]);

        refresh(&layout, &bridge).await.unwrap();

        assert_eq!(bridge.asked(), []);
    }

    #[tokio::test]
    async fn should_fetch_recent_then_the_rest_of_six_months_on_a_first_sync() {
        let layout = layout_with("first-sync", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![("A1", "Checking", vec![])]))]);

        refresh(&layout, &bridge).await.unwrap();

        assert_eq!(
            bridge.asked(),
            [
                window(days_ago(90), None, &[]),
                window(days_ago(180), Some(days_ago(90)), &["A1"]),
            ]
        );
    }

    #[tokio::test]
    async fn should_store_transactions_from_every_request() {
        let layout = layout_with("stores", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![
            Ok(set_with(vec![(
                "A1",
                "Checking",
                vec![sf_transaction("recent", days_ago(3), "-4.00")],
            )])),
            Ok(set_with(vec![(
                "A1",
                "Checking",
                vec![sf_transaction("older", days_ago(120), "-9.00")],
            )])),
        ]);

        refresh(&layout, &bridge).await.unwrap();

        assert_eq!(stored_ids(&layout), ["older", "recent"]);
    }

    #[tokio::test]
    async fn should_count_the_transactions_added() {
        let layout = layout_with("counts", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![
            Ok(set_with(vec![(
                "A1",
                "Checking",
                vec![sf_transaction("recent", days_ago(3), "-4.00")],
            )])),
            Ok(set_with(vec![(
                "A1",
                "Checking",
                vec![sf_transaction("older", days_ago(120), "-9.00")],
            )])),
        ]);

        let outcome = refresh(&layout, &bridge).await.unwrap();

        assert_eq!(outcome.transactions_added, 2);
    }

    #[tokio::test]
    async fn should_record_six_months_fetched_after_a_first_sync() {
        let layout = layout_with("records", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![("A1", "Checking", vec![])]))]);

        refresh(&layout, &bridge).await.unwrap();

        let synced = sync_state(&layout).sources["checking"];
        assert_eq!((synced.from, synced.through), (days_ago(180), today()));
    }

    #[tokio::test]
    async fn should_ask_only_for_recent_days_once_history_is_in() {
        let layout = layout_with("incremental", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![("A1", "Checking", vec![])]))]);
        refresh(&layout, &bridge).await.unwrap();
        let second = FakeBridge::answering(vec![Ok(set_with(vec![("A1", "Checking", vec![])]))]);

        refresh(&layout, &second).await.unwrap();

        assert_eq!(second.asked(), [window(days_ago(7), None, &[])]);
    }

    #[tokio::test]
    async fn should_link_an_account_opened_since_and_backfill_it_alone() {
        let layout = layout_with("auto-link", &[simplefin_source("checking", "A1")]);
        let first = FakeBridge::answering(vec![Ok(set_with(vec![("A1", "Checking", vec![])]))]);
        refresh(&layout, &first).await.unwrap();
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![
            ("A1", "Checking", vec![]),
            ("N1", "Savings", vec![]),
        ]))]);

        refresh(&layout, &bridge).await.unwrap();

        assert_eq!(
            bridge.asked()[1..],
            [
                window(days_ago(97), Some(days_ago(7)), &["N1"]),
                window(days_ago(180), Some(days_ago(97)), &["N1"]),
            ]
        );
    }

    #[tokio::test]
    async fn should_report_accounts_it_linked() {
        let layout = layout_with("auto-link-report", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![
            ("A1", "Checking", vec![]),
            ("N1", "Savings", vec![]),
        ]))]);

        let outcome = refresh(&layout, &bridge).await.unwrap();

        assert_eq!(outcome.added_sources, ["Chase Bank Savings"]);
    }

    #[tokio::test]
    async fn should_store_transactions_for_an_account_it_just_linked() {
        let layout = layout_with("auto-link-store", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![
            ("A1", "Checking", vec![]),
            (
                "N1",
                "Savings",
                vec![sf_transaction("new", days_ago(2), "5.00")],
            ),
        ]))]);

        refresh(&layout, &bridge).await.unwrap();

        assert_eq!(stored_ids(&layout), ["new"]);
    }

    #[tokio::test]
    async fn should_warn_and_retry_later_when_a_history_request_fails() {
        let layout = layout_with("history-fails", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![
            Ok(set_with(vec![("A1", "Checking", vec![])])),
            Err("SimpleFIN request budget used up".into()),
        ]);

        let outcome = refresh(&layout, &bridge).await.unwrap();

        assert!(outcome
            .warning
            .is_some_and(|warning| warning.contains("budget")));
    }

    #[tokio::test]
    async fn should_leave_failed_history_to_fetch_next_time() {
        let layout = layout_with("history-retry", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![
            Ok(set_with(vec![("A1", "Checking", vec![])])),
            Err("SimpleFIN request budget used up".into()),
        ]);

        refresh(&layout, &bridge).await.unwrap();

        assert_eq!(sync_state(&layout).sources["checking"].from, days_ago(90));
    }

    #[tokio::test]
    async fn should_fill_the_gap_after_a_long_absence() {
        let layout = layout_with("long-absence", &[simplefin_source("checking", "A1")]);
        let mut state = SyncState::default();
        state.sources.insert(
            "checking".into(),
            obol_core::SourceSync {
                from: days_ago(300),
                through: days_ago(120),
            },
        );
        save_sync_state(&layout.simplefin_sync, &state).unwrap();
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![("A1", "Checking", vec![])]))]);

        refresh(&layout, &bridge).await.unwrap();

        assert_eq!(
            bridge.asked(),
            [
                window(days_ago(90), None, &[]),
                window(days_ago(127), Some(days_ago(90)), &["A1"]),
            ]
        );
    }

    #[tokio::test]
    async fn should_stop_fetching_history_after_a_failure() {
        // Two history requests for a new account; a later one succeeding
        // after an earlier failure would record a hole as fetched.
        let layout = layout_with("history-stops", &[simplefin_source("checking", "A1")]);
        let first = FakeBridge::answering(vec![Ok(set_with(vec![("A1", "Checking", vec![])]))]);
        refresh(&layout, &first).await.unwrap();
        let both = set_with(vec![("A1", "Checking", vec![]), ("N1", "Savings", vec![])]);
        let bridge = FakeBridge::answering(vec![
            Ok(both.clone()),
            Err("SimpleFIN request budget used up".into()),
            Ok(both),
        ]);

        refresh(&layout, &bridge).await.unwrap();

        assert_eq!(bridge.asked().len(), 2);
    }

    #[tokio::test]
    async fn should_stop_after_a_failed_recent_request() {
        let layout = layout_with("recent-fails", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![Err("access denied".into())]);

        refresh(&layout, &bridge).await.unwrap();

        assert_eq!(bridge.asked().len(), 1);
    }

    #[tokio::test]
    async fn should_record_nothing_after_a_failed_recent_request() {
        let layout = layout_with("recent-fails-state", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![Err("access denied".into())]);

        refresh(&layout, &bridge).await.unwrap();

        assert_eq!(sync_state(&layout), SyncState::default());
    }

    #[tokio::test]
    async fn should_report_a_malformed_sync_file() {
        let layout = layout_with("malformed", &[simplefin_source("checking", "A1")]);
        fs::write(&layout.simplefin_sync, "not json").unwrap();
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![]))]);

        assert!(refresh(&layout, &bridge).await.is_err());
    }

    // --- load_earlier ---

    async fn earlier(layout: &StorageLayout, bridge: &FakeBridge) -> Result<EarlierResult, String> {
        load_earlier(layout, bridge, &Mutex::new(()), today()).await
    }

    async fn synced_layout(name: &str) -> StorageLayout {
        let layout = layout_with(name, &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![("A1", "Checking", vec![])]))]);
        refresh(&layout, &bridge).await.unwrap();
        layout
    }

    #[tokio::test]
    async fn should_refuse_to_load_earlier_before_anything_is_fetched() {
        let layout = layout_with("earlier-nothing", &[simplefin_source("checking", "A1")]);
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![]))]);

        assert!(earlier(&layout, &bridge).await.is_err());
    }

    #[tokio::test]
    async fn should_ask_for_the_90_days_before_the_earliest_day_fetched() {
        let layout = synced_layout("earlier-window").await;
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![("A1", "Checking", vec![])]))]);

        earlier(&layout, &bridge).await.unwrap();

        assert_eq!(
            bridge.asked(),
            [window(days_ago(270), Some(days_ago(180)), &[])]
        );
    }

    #[tokio::test]
    async fn should_report_what_loading_earlier_added() {
        let layout = synced_layout("earlier-result").await;
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![(
            "A1",
            "Checking",
            vec![sf_transaction("old", days_ago(200), "-3.00")],
        )]))]);

        let result = earlier(&layout, &bridge).await.unwrap();

        assert_eq!(
            result,
            EarlierResult {
                added: 1,
                history_from: format_iso_date(days_ago(270)),
            }
        );
    }

    #[tokio::test]
    async fn should_record_the_earlier_range_it_loaded() {
        let layout = synced_layout("earlier-records").await;
        let bridge = FakeBridge::answering(vec![Ok(set_with(vec![("A1", "Checking", vec![])]))]);

        earlier(&layout, &bridge).await.unwrap();

        assert_eq!(sync_state(&layout).sources["checking"].from, days_ago(270));
    }

    #[tokio::test]
    async fn should_keep_the_history_range_when_loading_earlier_fails() {
        let layout = synced_layout("earlier-fails").await;
        let bridge = FakeBridge::answering(vec![Err("SimpleFIN request budget used up".into())]);

        let _ = earlier(&layout, &bridge).await;

        assert_eq!(sync_state(&layout).sources["checking"].from, days_ago(180));
    }

    #[tokio::test]
    async fn should_say_why_loading_earlier_failed() {
        let layout = synced_layout("earlier-fails-message").await;
        let bridge = FakeBridge::answering(vec![Err("SimpleFIN request budget used up".into())]);

        let err = earlier(&layout, &bridge).await.unwrap_err();

        assert!(err.contains("budget"));
    }
}
