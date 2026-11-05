//! Keeping SimpleFIN transactions in step with the Bridge (#38, D44): which
//! days to ask for, and what each linked account has already been fetched
//! for. Pure, apart from the small state file; the requests themselves are
//! the caller's, through the shared provider.
//!
//! Every Refresh asks for balances and recent transactions in one request,
//! starting a few days before the last day already fetched so late
//! postings arrive. An account seen for the first time is backfilled six
//! months, and the user can load earlier history 90 days at a time.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io;
use std::path::Path;

use serde::{Deserialize, Serialize};
use thiserror::Error;
use time::{Date, Duration, OffsetDateTime};

use crate::atomic_write::write_atomically;
use crate::simplefin::{AccountSet, TransactionWindow};
use crate::simplefin_provider::sanitize;
use crate::transaction::{iso_date, Transaction};
use crate::SourceConfig;

/// How far back an account's first sync reaches: six months.
pub const FIRST_SYNC_DAYS: i64 = 180;
/// The Bridge serves at most 90 days of transactions per request.
pub const WINDOW_DAYS: i64 = 90;
/// Re-fetched at the start of every Refresh, so a transaction that posts a
/// few days late is still picked up.
pub const OVERLAP_DAYS: i64 = 7;

/// The days already fetched for one source, both inclusive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct SourceSync {
    #[serde(with = "iso_date")]
    pub from: Date,
    #[serde(with = "iso_date")]
    pub through: Date,
}

/// `simplefin_sync.json`: source id → the days fetched for it.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    #[serde(default)]
    pub sources: BTreeMap<String, SourceSync>,
}

/// Days to fetch: `start` inclusive, `end` exclusive (`None` reaches to
/// now), for `source_ids`, or for every SimpleFIN source when it's empty.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncWindow {
    pub start: Date,
    pub end: Option<Date>,
    pub source_ids: Vec<String>,
}

impl SyncWindow {
    /// The Bridge request for this window: dates as Unix seconds at UTC
    /// midnight, and source ids as their Bridge account ids.
    pub fn request(&self, sources: &[SourceConfig]) -> TransactionWindow {
        TransactionWindow {
            start: unix_midnight(self.start),
            end: self.end.map(unix_midnight),
            accounts: self
                .source_ids
                .iter()
                .filter_map(|id| sources.iter().find(|source| &source.id == id))
                .filter_map(bridge_account_id)
                .map(String::from)
                .collect(),
        }
    }
}

fn unix_midnight(date: Date) -> i64 {
    date.midnight().assume_utc().unix_timestamp()
}

fn bridge_account_id(source: &SourceConfig) -> Option<&str> {
    source.provider_config.get("simplefin_account_id")?.as_str()
}

fn simplefin_sources(sources: &[SourceConfig]) -> impl Iterator<Item = &SourceConfig> {
    sources
        .iter()
        .filter(|source| source.provider == "simplefin")
}

#[derive(Debug, Error)]
pub enum SyncStateError {
    #[error("simplefin_sync.json could not be read: {0}")]
    Io(#[from] io::Error),
    #[error("simplefin_sync.json could not be parsed: {0}")]
    Parse(#[from] serde_json::Error),
}

/// Missing means nothing has been fetched yet.
pub fn load_sync_state(path: &Path) -> Result<SyncState, SyncStateError> {
    match fs::read(path) {
        Ok(bytes) => Ok(serde_json::from_slice(&bytes)?),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(SyncState::default()),
        Err(err) => Err(err.into()),
    }
}

pub fn save_sync_state(path: &Path, state: &SyncState) -> Result<(), SyncStateError> {
    Ok(write_atomically(path, &serde_json::to_vec_pretty(state)?)?)
}

/// The window every Refresh fetches, for every account: from `OVERLAP_DAYS`
/// before the oldest day any SimpleFIN source has been fetched through, or
/// `WINDOW_DAYS` back before anything has, and never further back than one
/// request can reach.
pub fn recent_window(state: &SyncState, sources: &[SourceConfig], today: Date) -> SyncWindow {
    let reach = today - Duration::days(WINDOW_DAYS);
    let oldest = simplefin_sources(sources)
        .filter_map(|source| state.sources.get(&source.id))
        .map(|synced| synced.through)
        .min();
    SyncWindow {
        start: oldest.map_or(reach, |through| {
            (through - Duration::days(OVERLAP_DAYS)).max(reach)
        }),
        end: None,
        source_ids: Vec::new(),
    }
}

/// What the recent window, starting at `recent_start`, leaves out: six
/// months back for a source never fetched or whose backfill stopped short,
/// and the days between an old fetch and `recent_start` after a long gap.
/// Split into requests of at most `WINDOW_DAYS`, newest first, so a failure
/// partway leaves a source's `from` where the next Refresh can resume.
pub fn history_windows(
    state: &SyncState,
    sources: &[SourceConfig],
    today: Date,
    recent_start: Date,
) -> Vec<SyncWindow> {
    let six_months = today - Duration::days(FIRST_SYNC_DAYS);
    // One request per distinct range, listing every source that needs it.
    // An empty or inverted range splits into no requests below.
    let mut ranges: BTreeMap<(Date, Date), Vec<String>> = BTreeMap::new();
    let mut want = |start: Date, end: Date, id: &str| {
        ranges.entry((start, end)).or_default().push(id.to_string());
    };
    for source in simplefin_sources(sources) {
        match state.sources.get(&source.id) {
            None => want(six_months, recent_start, &source.id),
            Some(synced) => {
                want(six_months, synced.from, &source.id);
                want(
                    synced.through - Duration::days(OVERLAP_DAYS),
                    recent_start,
                    &source.id,
                );
            }
        }
    }

    let mut windows = Vec::new();
    for ((start, end), source_ids) in ranges {
        let mut chunk_end = end;
        while chunk_end > start {
            let chunk_start = (chunk_end - Duration::days(WINDOW_DAYS)).max(start);
            windows.push(SyncWindow {
                start: chunk_start,
                end: Some(chunk_end),
                source_ids: source_ids.clone(),
            });
            chunk_end = chunk_start;
        }
    }
    windows.sort_by_key(|window| std::cmp::Reverse(window.end));
    windows
}

/// "Load earlier": the `WINDOW_DAYS` before the earliest day fetched for any
/// SimpleFIN source. `None` before anything has been fetched.
pub fn earlier_window(state: &SyncState, sources: &[SourceConfig]) -> Option<SyncWindow> {
    let earliest = simplefin_sources(sources)
        .filter_map(|source| state.sources.get(&source.id))
        .map(|synced| synced.from)
        .min()?;
    Some(SyncWindow {
        start: earliest - Duration::days(WINDOW_DAYS),
        end: Some(earliest),
        source_ids: Vec::new(),
    })
}

/// Marks `window` fetched for the sources it covers.
pub fn record_window(
    state: &mut SyncState,
    window: &SyncWindow,
    sources: &[SourceConfig],
    today: Date,
) {
    let through = window.end.map_or(today, |end| end - Duration::days(1));
    let covered: BTreeSet<&str> = window.source_ids.iter().map(String::as_str).collect();
    for source in simplefin_sources(sources)
        .filter(|source| covered.is_empty() || covered.contains(source.id.as_str()))
    {
        state
            .sources
            .entry(source.id.clone())
            .and_modify(|synced| {
                synced.from = synced.from.min(window.start);
                synced.through = synced.through.max(through);
            })
            .or_insert(SourceSync {
                from: window.start,
                through,
            });
    }
}

/// The posted transactions in `set` for accounts linked to a SimpleFIN
/// source, each under that source's id. Pending ones, ones with no posting
/// time, and ones whose amount isn't a number are left out.
pub fn transactions_in(set: &AccountSet, sources: &[SourceConfig]) -> Vec<Transaction> {
    let source_for: BTreeMap<&str, &str> = simplefin_sources(sources)
        .filter_map(|source| Some((bridge_account_id(source)?, source.id.as_str())))
        .collect();
    set.accounts
        .iter()
        .filter_map(|account| Some((*source_for.get(account.id.as_str())?, account)))
        .flat_map(|(source_id, account)| {
            account
                .transactions
                .iter()
                .filter(|transaction| !transaction.pending && transaction.posted > 0)
                .filter_map(move |transaction| {
                    Some(Transaction {
                        source_id: source_id.to_string(),
                        id: transaction.id.clone(),
                        posted: OffsetDateTime::from_unix_timestamp(transaction.posted)
                            .ok()?
                            .date(),
                        amount: transaction
                            .amount
                            .trim()
                            .parse::<f64>()
                            .ok()
                            .filter(|amount| amount.is_finite())?,
                        // Server text that's shown and stored, so strip
                        // control characters like every other Bridge string.
                        description: sanitize(&transaction.description),
                    })
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::simplefin::{SimpleFinAccount, SimpleFinConnection, SimpleFinTransaction};
    use crate::Category;
    use serde_json::json;
    use time::Month;

    fn day(year: i32, month: Month, d: u8) -> Date {
        Date::from_calendar_date(year, month, d).unwrap()
    }

    fn today() -> Date {
        day(2026, Month::November, 4)
    }

    fn days_ago(n: i64) -> Date {
        today() - Duration::days(n)
    }

    fn simplefin_source(id: &str, account_id: &str) -> SourceConfig {
        SourceConfig {
            id: id.into(),
            provider: "simplefin".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            institution: "Chase Bank".into(),
            account_salt: "salt".into(),
            provider_config: json!({ "simplefin_account_id": account_id }),
        }
    }

    fn other_source(id: &str) -> SourceConfig {
        SourceConfig {
            provider: "manual".into(),
            provider_config: json!({}),
            ..simplefin_source(id, "")
        }
    }

    fn synced(entries: &[(&str, Date, Date)]) -> SyncState {
        SyncState {
            sources: entries
                .iter()
                .map(|(id, from, through)| {
                    (
                        id.to_string(),
                        SourceSync {
                            from: *from,
                            through: *through,
                        },
                    )
                })
                .collect(),
        }
    }

    fn window(start: Date, end: Option<Date>, ids: &[&str]) -> SyncWindow {
        SyncWindow {
            start,
            end,
            source_ids: ids.iter().map(|id| id.to_string()).collect(),
        }
    }

    // --- recent_window ---

    #[test]
    fn should_reach_one_request_back_on_a_first_sync() {
        let sources = [simplefin_source("checking", "A1")];

        let recent = recent_window(&SyncState::default(), &sources, today());

        assert_eq!(recent, window(days_ago(90), None, &[]));
    }

    #[test]
    fn should_start_an_overlap_before_the_oldest_day_fetched() {
        let sources = [simplefin_source("a", "A1"), simplefin_source("b", "B1")];
        let state = synced(&[
            ("a", days_ago(180), days_ago(1)),
            ("b", days_ago(180), days_ago(3)),
        ]);

        let recent = recent_window(&state, &sources, today());

        assert_eq!(recent.start, days_ago(10));
    }

    #[test]
    fn should_never_reach_further_back_than_one_request() {
        let sources = [simplefin_source("a", "A1")];
        let state = synced(&[("a", days_ago(400), days_ago(200))]);

        let recent = recent_window(&state, &sources, today());

        assert_eq!(recent.start, days_ago(90));
    }

    #[test]
    fn should_ignore_other_providers_when_placing_the_recent_window() {
        let sources = [simplefin_source("a", "A1"), other_source("cash")];
        let state = synced(&[
            ("a", days_ago(180), days_ago(1)),
            ("cash", days_ago(180), days_ago(60)),
        ]);

        let recent = recent_window(&state, &sources, today());

        assert_eq!(recent.start, days_ago(8));
    }

    // --- history_windows ---

    #[test]
    fn should_backfill_six_months_on_a_first_sync() {
        let sources = [simplefin_source("a", "A1"), simplefin_source("b", "B1")];

        let history = history_windows(&SyncState::default(), &sources, today(), days_ago(90));

        assert_eq!(
            history,
            [window(days_ago(180), Some(days_ago(90)), &["a", "b"])]
        );
    }

    #[test]
    fn should_backfill_a_new_account_alone_newest_first() {
        let sources = [simplefin_source("a", "A1"), simplefin_source("new", "N1")];
        let state = synced(&[("a", days_ago(180), days_ago(1))]);

        let history = history_windows(&state, &sources, today(), days_ago(8));

        assert_eq!(
            history,
            [
                window(days_ago(98), Some(days_ago(8)), &["new"]),
                window(days_ago(180), Some(days_ago(98)), &["new"]),
            ]
        );
    }

    #[test]
    fn should_resume_a_backfill_that_stopped_short() {
        let sources = [simplefin_source("a", "A1")];
        let state = synced(&[("a", days_ago(98), days_ago(1))]);

        let history = history_windows(&state, &sources, today(), days_ago(8));

        assert_eq!(history, [window(days_ago(180), Some(days_ago(98)), &["a"])]);
    }

    #[test]
    fn should_fetch_nothing_more_for_a_source_already_covered() {
        let sources = [simplefin_source("a", "A1")];
        let state = synced(&[("a", days_ago(180), days_ago(1))]);

        let history = history_windows(&state, &sources, today(), days_ago(8));

        assert_eq!(history, []);
    }

    #[test]
    fn should_fill_the_gap_after_a_long_absence() {
        let sources = [simplefin_source("a", "A1")];
        let state = synced(&[("a", days_ago(300), days_ago(120))]);

        let history = history_windows(&state, &sources, today(), days_ago(90));

        assert_eq!(history, [window(days_ago(127), Some(days_ago(90)), &["a"])]);
    }

    #[test]
    fn should_leave_other_providers_out_of_history() {
        let sources = [other_source("cash")];

        let history = history_windows(&SyncState::default(), &sources, today(), days_ago(90));

        assert_eq!(history, []);
    }

    // --- earlier_window ---

    #[test]
    fn should_offer_no_earlier_window_before_anything_is_fetched() {
        let sources = [simplefin_source("a", "A1")];

        assert_eq!(earlier_window(&SyncState::default(), &sources), None);
    }

    #[test]
    fn should_load_the_request_before_the_earliest_day_fetched() {
        let sources = [simplefin_source("a", "A1"), simplefin_source("b", "B1")];
        let state = synced(&[
            ("a", days_ago(180), days_ago(1)),
            ("b", days_ago(200), days_ago(1)),
        ]);

        let earlier = earlier_window(&state, &sources);

        assert_eq!(
            earlier,
            Some(window(days_ago(290), Some(days_ago(200)), &[]))
        );
    }

    #[test]
    fn should_ignore_other_providers_when_loading_earlier() {
        let sources = [simplefin_source("a", "A1"), other_source("cash")];
        let state = synced(&[
            ("a", days_ago(180), days_ago(1)),
            ("cash", days_ago(500), days_ago(1)),
        ]);

        let earlier = earlier_window(&state, &sources);

        assert_eq!(earlier.map(|w| w.end), Some(Some(days_ago(180))));
    }

    // --- record_window ---

    #[test]
    fn should_record_a_first_fetch_through_today() {
        let sources = [simplefin_source("a", "A1")];
        let mut state = SyncState::default();

        record_window(
            &mut state,
            &window(days_ago(90), None, &[]),
            &sources,
            today(),
        );

        assert_eq!(state, synced(&[("a", days_ago(90), today())]));
    }

    #[test]
    fn should_record_a_closed_window_through_the_day_before_its_end() {
        let sources = [simplefin_source("a", "A1")];
        let mut state = SyncState::default();

        record_window(
            &mut state,
            &window(days_ago(180), Some(days_ago(90)), &["a"]),
            &sources,
            today(),
        );

        assert_eq!(state, synced(&[("a", days_ago(180), days_ago(91))]));
    }

    #[test]
    fn should_widen_an_existing_record() {
        let sources = [simplefin_source("a", "A1")];
        let mut state = synced(&[("a", days_ago(90), days_ago(5))]);

        record_window(
            &mut state,
            &window(days_ago(180), Some(days_ago(90)), &[]),
            &sources,
            today(),
        );

        assert_eq!(state, synced(&[("a", days_ago(180), days_ago(5))]));
    }

    #[test]
    fn should_record_only_the_sources_a_window_lists() {
        let sources = [simplefin_source("a", "A1"), simplefin_source("b", "B1")];
        let mut state = SyncState::default();

        record_window(
            &mut state,
            &window(days_ago(90), None, &["b"]),
            &sources,
            today(),
        );

        assert_eq!(state, synced(&[("b", days_ago(90), today())]));
    }

    #[test]
    fn should_not_record_other_providers_for_an_unfiltered_window() {
        let sources = [simplefin_source("a", "A1"), other_source("cash")];
        let mut state = SyncState::default();

        record_window(
            &mut state,
            &window(days_ago(90), None, &[]),
            &sources,
            today(),
        );

        assert_eq!(state, synced(&[("a", days_ago(90), today())]));
    }

    // --- SyncWindow::request ---

    #[test]
    fn should_send_dates_as_utc_midnight_seconds() {
        let request = window(
            day(2026, Month::November, 1),
            Some(day(2026, Month::November, 2)),
            &[],
        )
        .request(&[]);

        assert_eq!(
            (request.start, request.end),
            (1_793_491_200, Some(1_793_577_600))
        );
    }

    #[test]
    fn should_send_bridge_account_ids_for_listed_sources() {
        let sources = [simplefin_source("a", "A1"), simplefin_source("b", "B1")];

        let request = window(days_ago(90), None, &["b"]).request(&sources);

        assert_eq!(request.accounts, ["B1"]);
    }

    #[test]
    fn should_send_no_account_filter_for_an_unfiltered_window() {
        let sources = [simplefin_source("a", "A1")];

        let request = window(days_ago(90), None, &[]).request(&sources);

        assert!(request.accounts.is_empty());
    }

    // --- transactions_in ---

    fn set_with(account_id: &str, transactions: Vec<SimpleFinTransaction>) -> AccountSet {
        AccountSet {
            errlist: vec![],
            connections: vec![SimpleFinConnection {
                conn_id: "c1".into(),
                name: "Chase Bank".into(),
            }],
            accounts: vec![SimpleFinAccount {
                id: account_id.into(),
                name: "Checking".into(),
                conn_id: "c1".into(),
                balance: "100.00".into(),
                transactions,
            }],
        }
    }

    fn sf_transaction(id: &str, posted: i64, amount: &str) -> SimpleFinTransaction {
        SimpleFinTransaction {
            id: id.into(),
            posted,
            amount: amount.into(),
            description: "CORNER CAFE".into(),
            pending: false,
        }
    }

    #[test]
    fn should_turn_a_posted_transaction_into_one_under_its_source() {
        let sources = [simplefin_source("checking", "A1")];
        // 2026-11-01 15:00 UTC.
        let set = set_with("A1", vec![sf_transaction("t1", 1_793_545_200, "-12.50")]);

        assert_eq!(
            transactions_in(&set, &sources),
            [Transaction {
                source_id: "checking".into(),
                id: "t1".into(),
                posted: day(2026, Month::November, 1),
                amount: -12.5,
                description: "CORNER CAFE".into(),
            }]
        );
    }

    #[test]
    fn should_leave_out_pending_transactions() {
        let sources = [simplefin_source("checking", "A1")];
        let pending = SimpleFinTransaction {
            pending: true,
            ..sf_transaction("t1", 1_793_545_200, "-12.50")
        };

        assert_eq!(
            transactions_in(&set_with("A1", vec![pending]), &sources),
            []
        );
    }

    #[test]
    fn should_leave_out_a_transaction_with_no_posting_time() {
        let sources = [simplefin_source("checking", "A1")];
        let set = set_with("A1", vec![sf_transaction("t1", 0, "-12.50")]);

        assert_eq!(transactions_in(&set, &sources), []);
    }

    #[test]
    fn should_leave_out_a_transaction_whose_amount_is_not_a_number() {
        let sources = [simplefin_source("checking", "A1")];
        let set = set_with("A1", vec![sf_transaction("t1", 1_793_545_200, "lots")]);

        assert_eq!(transactions_in(&set, &sources), []);
    }

    #[test]
    fn should_leave_out_a_transaction_whose_amount_is_not_finite() {
        let sources = [simplefin_source("checking", "A1")];
        let set = set_with("A1", vec![sf_transaction("t1", 1_793_545_200, "inf")]);

        assert_eq!(transactions_in(&set, &sources), []);
    }

    #[test]
    fn should_strip_control_characters_from_descriptions() {
        let sources = [simplefin_source("checking", "A1")];
        let transaction = SimpleFinTransaction {
            description: "CORNER\u{7} CAFE\u{1b}".into(),
            ..sf_transaction("t1", 1_793_545_200, "-1.00")
        };

        let stored = transactions_in(&set_with("A1", vec![transaction]), &sources);

        assert_eq!(stored[0].description, "CORNER CAFE");
    }

    #[test]
    fn should_leave_out_accounts_no_source_links() {
        let sources = [simplefin_source("checking", "A1")];
        let set = set_with("ZZ", vec![sf_transaction("t1", 1_793_545_200, "-12.50")]);

        assert_eq!(transactions_in(&set, &sources), []);
    }

    // --- state file ---

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "obol-simplefin-sync-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir.join("simplefin_sync.json")
    }

    #[test]
    fn should_read_an_empty_state_when_the_file_is_missing() {
        assert_eq!(
            load_sync_state(&temp_path("missing")).unwrap(),
            SyncState::default()
        );
    }

    #[test]
    fn should_round_trip_the_state() {
        let path = temp_path("round-trip");
        let state = synced(&[("a", days_ago(180), today())]);

        save_sync_state(&path, &state).unwrap();

        assert_eq!(load_sync_state(&path).unwrap(), state);
    }

    #[test]
    fn should_report_a_malformed_state_file() {
        let path = temp_path("malformed");
        fs::write(&path, "not json").unwrap();

        assert!(matches!(
            load_sync_state(&path),
            Err(SyncStateError::Parse(_))
        ));
    }
}
