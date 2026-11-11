//! The dashboard's view model (spec §16 v0.3a, D43) — the shape the
//! webview renders, assembled from a `Snapshot`.
//!
//! This exists because `obol-core`'s own types can't cross the Tauri
//! boundary as they are: `Snapshot`/`AccountRecord` do derive `Serialize`
//! and carry no PII by design (§11.1), but [`NetWorth`] derives neither,
//! and the net worth figure has to be *computed* rather than read off the
//! snapshot.
//!
//! What it deliberately is **not** is a second home for domain logic
//! (D43's "no logic rewrite"): the sign rule for net worth stays in
//! `calculate_net_worth_from_records` where D11 put it. This module only
//! groups and maps.

use obol_core::{
    calculate_net_worth_from_records, is_access_revoked, parse_iso_date, AccountRecord, Category,
    NetWorth, Snapshot, Status,
};
use serde::Serialize;
use std::collections::BTreeMap;

/// Everything the balances screen needs for one render.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DashboardView {
    pub created_at: String,
    pub net_worth: NetWorthView,
    pub assets: Vec<AccountView>,
    pub liabilities: Vec<AccountView>,
    /// Each group's total over status-OK balances only, matching the
    /// TUI's panel titles (`crates/cli/src/dashboard.rs`). Liabilities are
    /// positive amounts owed; the two reconcile with `net_worth` as
    /// assets minus liabilities.
    pub assets_subtotal: f64,
    pub liabilities_subtotal: f64,
    /// `None` on a first-ever run, or when either snapshot's net worth
    /// isn't computable — there's nothing meaningful to diff against.
    pub delta: Option<DeltaView>,
    /// Net worth per saved snapshot, oldest first, for the sparkline.
    /// Filled in by the dashboard loaders, which hold the history;
    /// [`build`] sees only two snapshots and leaves it empty.
    pub trend: Vec<TrendPoint>,
    /// A source failed because SimpleFIN no longer accepts the saved Access
    /// URL, so only a new Setup Token brings it back.
    pub reconnect_simplefin: bool,
}

/// One snapshot's net worth on the day it was taken.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TrendPoint {
    /// `YYYY-MM-DD`, as recorded in the snapshot.
    pub date: String,
    pub value: f64,
}

/// Internally tagged so the frontend branches on `kind` rather than on
/// a null check. §9.1 forbids rendering an all-failed run as `$0`, which
/// is indistinguishable from a genuine zero, so "no value" is a distinct
/// variant carrying why rather than an absent number.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NetWorthView {
    Computed { value: f64 },
    /// This run returned nothing, so this is the newest figure from an
    /// earlier snapshot, labelled with its day rather than passed off as
    /// current. Set by [`use_last_known_net_worth`], never by [`build`].
    LastKnown {
        value: f64,
        /// `YYYY-MM-DD`.
        as_of: String,
        total_sources: usize,
    },
    Unavailable { total_sources: usize },
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum StatusView {
    Ok,
    Error,
    /// D14's forward-compat fallback, carried through rather than
    /// collapsed into `Error` — a category this build doesn't understand
    /// isn't the same thing as a source that failed.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AccountView {
    pub source_id: String,
    pub institution: String,
    /// Your own name for the account, when you've set one. Filled in by
    /// [`apply_labels`]; [`build`] leaves it empty.
    pub label: Option<String>,
    pub account_type: String,
    pub balance: Option<f64>,
    pub currency: String,
    pub status: StatusView,
    /// Present only for a failed source; FR18 requires its panel show
    /// this rather than a blank.
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct DeltaView {
    pub amount: f64,
    pub since: String,
}

/// Assembles one render's worth of view model.
///
/// `previous` is the most recent snapshot from *before* this run, used
/// only for the since-delta — the same contract the TUI's dashboard
/// already has (`crates/cli/src/dashboard.rs`).
pub fn build(snapshot: &Snapshot, previous: Option<&Snapshot>) -> DashboardView {
    // Computed once and reused for the delta below — `NetWorth` is
    // `Copy`, and re-deriving it per use would invite the two figures
    // to disagree.
    let net_worth = calculate_net_worth_from_records(&snapshot.accounts);
    let (assets, liabilities) = partition(&snapshot.accounts);

    DashboardView {
        created_at: snapshot.created_at.clone(),
        delta: delta(net_worth, previous),
        net_worth: net_worth.into(),
        assets_subtotal: subtotal(&assets),
        liabilities_subtotal: subtotal(&liabilities),
        assets,
        liabilities,
        trend: Vec::new(),
        reconnect_simplefin: snapshot.accounts.iter().any(|record| {
            record.status() == Status::Error
                && record.error_message().is_some_and(is_access_revoked)
        }),
    }
}

/// When this run has no net worth at all, shows the newest one the trend
/// holds instead of a failure. Needs the trend, so the loaders call it after
/// filling that in. With nothing earlier, or no sources to have failed, the
/// figure stays unavailable.
pub fn use_last_known_net_worth(view: &mut DashboardView) {
    let NetWorthView::Unavailable { total_sources } = view.net_worth else {
        return;
    };
    let Some(last) = view.trend.last().filter(|_| total_sources > 0) else {
        return;
    };
    view.net_worth = NetWorthView::LastKnown {
        value: last.value,
        as_of: last.date.clone(),
        total_sources,
    };
}

/// Puts your own names on the accounts, then re-sorts so a labelled
/// account lands by its name among the others at the same institution.
pub fn apply_labels(view: &mut DashboardView, labels: &BTreeMap<String, String>) {
    for group in [&mut view.assets, &mut view.liabilities] {
        for account in group.iter_mut() {
            account.label = labels.get(&account.source_id).cloned();
        }
        sort_accounts(group);
    }
}

/// Sources are fetched concurrently and recorded as they finish, so the
/// snapshot's order changes run to run; a fixed order keeps rows still.
fn sort_accounts(group: &mut [AccountView]) {
    group.sort_by_cached_key(|a| {
        (
            a.institution.to_lowercase(),
            a.label.as_deref().unwrap_or_default().to_lowercase(),
            a.account_type.clone(),
            a.source_id.clone(),
        )
    });
}

/// Net worth of each snapshot, oldest first. A snapshot whose net worth
/// can't be computed (every source failed) is skipped rather than drawn
/// as a drop to zero, the same rule [`delta`] applies.
///
/// Covers the year ending at the newest snapshot, not at today's date, so
/// a long gap between refreshes doesn't empty the chart.
pub fn trend<'a>(newest_first: impl Iterator<Item = &'a Snapshot>) -> Vec<TrendPoint> {
    let snapshots: Vec<&Snapshot> = newest_first.collect();
    let Some(newest) = snapshots.first().and_then(|s| snapshot_day(s)) else {
        return Vec::new();
    };
    let cutoff = newest - time::Duration::days(TREND_DAYS);

    snapshots
        .iter()
        .rev()
        .filter_map(|snapshot| {
            let day = snapshot_day(snapshot)?;
            if day < cutoff {
                return None;
            }
            match calculate_net_worth_from_records(&snapshot.accounts) {
                NetWorth::Computed(value) => Some(TrendPoint {
                    date: day.to_string(),
                    value: to_cents(value),
                }),
                NetWorth::Unavailable { .. } => None,
            }
        })
        .collect()
}

/// How far back the net worth chart reaches.
const TREND_DAYS: i64 = 365;

/// The calendar day a snapshot was taken, as recorded in its timestamp.
fn snapshot_day(snapshot: &Snapshot) -> Option<time::Date> {
    parse_iso_date(snapshot.created_at.get(..10)?)
}

impl From<NetWorth> for NetWorthView {
    fn from(net_worth: NetWorth) -> Self {
        match net_worth {
            NetWorth::Computed(value) => NetWorthView::Computed { value },
            NetWorth::Unavailable { total_sources } => NetWorthView::Unavailable { total_sources },
        }
    }
}

fn partition(records: &[AccountRecord]) -> (Vec<AccountView>, Vec<AccountView>) {
    let mut assets = Vec::new();
    let mut liabilities = Vec::new();

    for record in records {
        match record.category() {
            Category::Liability => liabilities.push(account_view(record)),
            // `Unknown` (D14's forward-compat fallback) groups with the
            // assets, the same side `calculate_net_worth_from_records`
            // already assigns it — so the two groups always reconcile
            // with the figure shown above them.
            Category::Asset | Category::Unknown => assets.push(account_view(record)),
        }
    }

    sort_accounts(&mut assets);
    sort_accounts(&mut liabilities);

    (assets, liabilities)
}

/// Filters on status rather than on whether a balance is present: a
/// failed or unrecognized fetch can still carry a stale figure, which
/// isn't one to add in. Same rule as the TUI's panel titles.
fn subtotal(accounts: &[AccountView]) -> f64 {
    accounts
        .iter()
        .filter(|account| account.status == StatusView::Ok)
        .filter_map(|account| account.balance)
        // Folded from +0.0 rather than `.sum()`, whose f64 identity is
        // -0.0: an empty group would otherwise display as "-$0.00".
        .fold(0.0, |total, balance| total + balance)
}

fn account_view(record: &AccountRecord) -> AccountView {
    AccountView {
        source_id: record.source_id().to_string(),
        institution: record.institution().to_string(),
        label: None,
        account_type: record.account_type().to_string(),
        balance: record.balance(),
        currency: record.currency().to_string(),
        status: match record.status() {
            Status::Ok => StatusView::Ok,
            Status::Error => StatusView::Error,
            Status::Unknown => StatusView::Unknown,
        },
        error_message: record.error_message().map(str::to_string),
    }
}

/// Rounds to the cent. Accounts are saved in whatever order their fetches
/// finish, so the same balances can sum to totals that differ in the 15th
/// decimal; unrounded, that reads as "−$0.00 since" and draws a slope.
/// The `+ 0.0` turns a rounded -0.0 into plain zero.
fn to_cents(amount: f64) -> f64 {
    (amount * 100.0).round() / 100.0 + 0.0
}

fn delta(current: NetWorth, previous: Option<&Snapshot>) -> Option<DeltaView> {
    let previous = previous?;

    // Both sides must be real figures: diffing against a run where every
    // source failed would report the whole balance as a gain.
    let (NetWorth::Computed(current), NetWorth::Computed(before)) = (
        current,
        calculate_net_worth_from_records(&previous.accounts),
    ) else {
        return None;
    };

    Some(DeltaView {
        amount: to_cents(current - before),
        since: previous.created_at.clone(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{asset, liability, snapshot_at};
    use serde_json::{json, Value};

    const NOW: &str = "2026-10-26T09:00:00-07:00";
    const EARLIER: &str = "2026-10-19T09:00:00-07:00";

    fn snapshot_of(accounts: Value) -> Snapshot {
        snapshot_at(NOW, accounts)
    }

    #[test]
    fn should_serialize_computed_net_worth_as_tagged_value() {
        let view = build(&snapshot_of(json!([asset("chase", 1000.0)])), None);

        assert_eq!(
            serde_json::to_value(&view.net_worth).unwrap(),
            json!({ "kind": "computed", "value": 1000.0 })
        );
    }

    #[test]
    fn should_serialize_unavailable_net_worth_with_total_sources() {
        let failed = json!({
            "account_key": "sha256:a",
            "source_id": "chase",
            "institution": "Chase",
            "category": "asset",
            "type": "checking",
            "balance": null,
            "currency": "USD",
            "status": "error",
            "error_message": "boom",
        });

        let view = build(&snapshot_of(json!([failed])), None);

        assert_eq!(
            serde_json::to_value(&view.net_worth).unwrap(),
            json!({ "kind": "unavailable", "total_sources": 1 })
        );
    }

    #[test]
    fn should_report_unavailable_for_empty_snapshot() {
        let view = build(&snapshot_of(json!([])), None);

        assert_eq!(
            view.net_worth,
            NetWorthView::Unavailable { total_sources: 0 }
        );
    }

    #[test]
    fn should_group_accounts_into_assets_and_liabilities() {
        let view = build(
            &snapshot_of(json!([asset("chase", 1000.0), liability("apple", 200.0)])),
            None,
        );

        let grouped: (Vec<&str>, Vec<&str>) = (
            view.assets.iter().map(|a| a.source_id.as_str()).collect(),
            view.liabilities
                .iter()
                .map(|a| a.source_id.as_str())
                .collect(),
        );
        assert_eq!(grouped, (vec!["chase"], vec!["apple"]));
    }

    fn account(source_id: &str, institution: &str, account_type: &str, category: &str) -> Value {
        json!({
            "account_key": format!("sha256:{source_id}"),
            "source_id": source_id,
            "institution": institution,
            "category": category,
            "type": account_type,
            "balance": 100.0,
            "currency": "USD",
            "status": "ok",
        })
    }

    /// One asset per balance, in the order given. Summed in that order,
    /// `[0.1, 0.2, 0.3]` and `[0.3, 0.2, 0.1]` differ by about 1e-16.
    fn assets_in_order(created_at: &str, balances: &[f64]) -> Snapshot {
        let accounts: Vec<Value> = balances
            .iter()
            .enumerate()
            .map(|(i, balance)| asset(&format!("a{i}"), *balance))
            .collect();
        snapshot_at(created_at, json!(accounts))
    }

    #[test]
    fn should_report_no_change_when_only_the_sum_order_differs() {
        let now = assets_in_order(NOW, &[0.1, 0.2, 0.3]);
        let before = assets_in_order(EARLIER, &[0.3, 0.2, 0.1]);

        let delta = build(&now, Some(&before)).delta.unwrap();

        assert_eq!(delta.amount, 0.0);
    }

    #[test]
    fn should_report_a_zero_change_as_positive_zero() {
        // Noise of about -1e-16: the "−$0.00 since" case.
        let now = assets_in_order(NOW, &[0.3, 0.2, 0.1]);
        let before = assets_in_order(EARLIER, &[0.1, 0.2, 0.3]);

        let delta = build(&now, Some(&before)).delta.unwrap();

        assert!(delta.amount.is_sign_positive());
    }

    #[test]
    fn should_round_a_change_to_the_cent() {
        let now = assets_in_order(NOW, &[1000.004]);
        let before = assets_in_order(EARLIER, &[1000.0]);

        let delta = build(&now, Some(&before)).delta.unwrap();

        assert_eq!(delta.amount, 0.0);
    }

    #[test]
    fn should_keep_a_real_change_to_the_cent() {
        let now = assets_in_order(NOW, &[1000.12]);
        let before = assets_in_order(EARLIER, &[1000.0]);

        let delta = build(&now, Some(&before)).delta.unwrap();

        assert_eq!(delta.amount, 0.12);
    }

    #[test]
    fn should_draw_equal_balances_as_equal_trend_values() {
        let older = assets_in_order(EARLIER, &[0.3, 0.2, 0.1]);
        let newer = assets_in_order(NOW, &[0.1, 0.2, 0.3]);

        let values: Vec<f64> = trend([&newer, &older].into_iter())
            .iter()
            .map(|p| p.value)
            .collect();

        assert_eq!(values[0], values[1]);
    }

    fn snap(created_at: &str, balance: f64) -> Snapshot {
        snapshot_at(created_at, json!([asset("chase", balance)]))
    }

    #[test]
    fn should_list_trend_oldest_first_from_newest_first_input() {
        let newest = snap("2026-10-26T09:00:00-07:00", 300.0);
        let older = snap("2026-10-19T09:00:00-07:00", 200.0);
        let oldest = snap("2026-10-12T09:00:00-07:00", 100.0);

        assert_eq!(
            trend([&newest, &older, &oldest].into_iter()),
            vec![
                point("2026-10-12", 100.0),
                point("2026-10-19", 200.0),
                point("2026-10-26", 300.0),
            ]
        );
    }

    fn point(date: &str, value: f64) -> TrendPoint {
        TrendPoint {
            date: date.to_string(),
            value,
        }
    }

    #[test]
    fn should_keep_a_snapshot_exactly_a_year_before_the_newest() {
        let newest = snap("2026-10-26T09:00:00-07:00", 300.0);
        let edge = snap("2025-10-26T09:00:00-07:00", 100.0);

        assert_eq!(trend([&newest, &edge].into_iter()).len(), 2);
    }

    #[test]
    fn should_drop_a_snapshot_more_than_a_year_before_the_newest() {
        let newest = snap("2026-10-26T09:00:00-07:00", 300.0);
        let too_old = snap("2025-10-25T09:00:00-07:00", 100.0);

        assert_eq!(
            trend([&newest, &too_old].into_iter()),
            vec![point("2026-10-26", 300.0)]
        );
    }

    #[test]
    fn should_skip_a_snapshot_whose_date_cannot_be_read() {
        let newest = snap("2026-10-26T09:00:00-07:00", 300.0);
        let garbled = snap("sometime last week", 100.0);

        assert_eq!(
            trend([&newest, &garbled].into_iter()),
            vec![point("2026-10-26", 300.0)]
        );
    }

    #[test]
    fn should_skip_snapshots_without_a_computable_net_worth_in_the_trend() {
        let failed = snapshot_of(json!([{
            "account_key": "sha256:a", "source_id": "chase", "institution": "Chase",
            "category": "asset", "type": "checking", "balance": null,
            "currency": "USD", "status": "error", "error_message": "boom",
        }]));
        let good = snap(EARLIER, 100.0);

        assert_eq!(
            trend([&failed, &good].into_iter()),
            vec![point("2026-10-19", 100.0)]
        );
    }

    #[test]
    fn should_have_an_empty_trend_without_snapshots() {
        assert_eq!(trend(std::iter::empty()), Vec::<TrendPoint>::new());
    }

    fn failed_snapshot(message: &str) -> Snapshot {
        snapshot_of(json!([{
            "account_key": "sha256:a", "source_id": "chase", "institution": "Chase",
            "category": "asset", "type": "checking", "balance": null,
            "currency": "USD", "status": "error", "error_message": message,
        }]))
    }

    /// A view of an all-failed run with one earlier good snapshot behind it.
    fn failed_run_after_good_one() -> DashboardView {
        let failed = failed_snapshot("boom");
        let mut view = build(&failed, None);
        view.trend = trend([&failed, &snap(EARLIER, 100.0)].into_iter());
        view
    }

    #[test]
    fn should_fall_back_to_the_last_known_net_worth_when_this_run_has_none() {
        let mut view = failed_run_after_good_one();

        use_last_known_net_worth(&mut view);

        assert_eq!(
            view.net_worth,
            NetWorthView::LastKnown {
                value: 100.0,
                as_of: "2026-10-19".into(),
                total_sources: 1
            }
        );
    }

    #[test]
    fn should_serialize_last_known_net_worth_as_a_tagged_value() {
        let mut view = failed_run_after_good_one();
        use_last_known_net_worth(&mut view);

        assert_eq!(
            serde_json::to_value(&view.net_worth).unwrap(),
            json!({
                "kind": "last_known", "value": 100.0,
                "as_of": "2026-10-19", "total_sources": 1
            })
        );
    }

    #[test]
    fn should_stay_unavailable_when_no_net_worth_was_ever_known() {
        let mut view = build(&failed_snapshot("boom"), None);

        use_last_known_net_worth(&mut view);

        assert_eq!(
            view.net_worth,
            NetWorthView::Unavailable { total_sources: 1 }
        );
    }

    #[test]
    fn should_stay_unavailable_when_no_sources_are_configured() {
        let mut view = build(&snapshot_of(json!([])), None);
        view.trend = vec![point("2026-10-19", 100.0)];

        use_last_known_net_worth(&mut view);

        assert_eq!(
            view.net_worth,
            NetWorthView::Unavailable { total_sources: 0 }
        );
    }

    #[test]
    fn should_keep_a_computed_net_worth_over_an_older_one() {
        let mut view = build(&snap(NOW, 300.0), None);
        view.trend = vec![point("2026-10-19", 100.0)];

        use_last_known_net_worth(&mut view);

        assert_eq!(view.net_worth, NetWorthView::Computed { value: 300.0 });
    }

    const REVOKED: &str =
        "authentication failed: access denied (HTTP 401): the Access URL is invalid or was revoked";

    #[test]
    fn should_ask_to_reconnect_when_the_access_url_was_revoked() {
        let view = build(&failed_snapshot(REVOKED), None);

        assert!(view.reconnect_simplefin);
    }

    #[test]
    fn should_not_ask_to_reconnect_for_other_failures() {
        let view = build(&failed_snapshot("operation timed out"), None);

        assert!(!view.reconnect_simplefin);
    }

    #[test]
    fn should_not_ask_to_reconnect_when_every_source_is_fine() {
        let view = build(&snap(NOW, 300.0), None);

        assert!(!view.reconnect_simplefin);
    }

    fn labelled(accounts: Value, labels: &[(&str, &str)]) -> DashboardView {
        let mut view = build(&snapshot_of(accounts), None);
        let labels = labels
            .iter()
            .map(|(id, label)| (id.to_string(), label.to_string()))
            .collect();
        apply_labels(&mut view, &labels);
        view
    }

    #[test]
    fn should_put_a_label_on_the_matching_account() {
        let view = labelled(
            json!([account("c1", "Chase", "checking", "asset")]),
            &[("c1", "Joint checking")],
        );

        assert_eq!(view.assets[0].label.as_deref(), Some("Joint checking"));
    }

    #[test]
    fn should_leave_unlabelled_accounts_without_a_label() {
        let view = labelled(
            json!([
                account("c1", "Chase", "checking", "asset"),
                account("c2", "Chase", "savings", "asset"),
            ]),
            &[("c1", "Joint checking")],
        );

        let other = view.assets.iter().find(|a| a.source_id == "c2").unwrap();
        assert_eq!(other.label, None);
    }

    #[test]
    fn should_label_liabilities_too() {
        let view = labelled(
            json!([account("g1", "Chase", "credit_card", "liability")]),
            &[("g1", "Travel card")],
        );

        assert_eq!(view.liabilities[0].label.as_deref(), Some("Travel card"));
    }

    #[test]
    fn should_ignore_a_label_for_an_unknown_account() {
        let view = labelled(
            json!([account("c1", "Chase", "checking", "asset")]),
            &[("gone", "Old account")],
        );

        assert_eq!(view.assets[0].label, None);
    }

    #[test]
    fn should_order_one_institutions_accounts_by_label() {
        let view = labelled(
            json!([
                account("a_first", "Chase", "checking", "asset"),
                account("z_last", "Chase", "checking", "asset"),
            ]),
            &[("a_first", "Zeta"), ("z_last", "Alpha")],
        );

        assert_eq!(asset_ids(&view), vec!["z_last", "a_first"]);
    }

    #[test]
    fn should_order_labels_ignoring_case() {
        let view = labelled(
            json!([
                account("a_first", "Chase", "checking", "asset"),
                account("z_last", "Chase", "checking", "asset"),
            ]),
            &[("a_first", "Beta"), ("z_last", "alpha")],
        );

        assert_eq!(asset_ids(&view), vec!["z_last", "a_first"]);
    }

    #[test]
    fn should_still_order_by_institution_before_label() {
        let view = labelled(
            json!([
                account("v", "Vanguard", "brokerage", "asset"),
                account("c", "Chase", "checking", "asset"),
            ]),
            &[("v", "Aardvark"), ("c", "Zebra")],
        );

        assert_eq!(asset_ids(&view), vec!["c", "v"]);
    }

    fn asset_ids(view: &DashboardView) -> Vec<&str> {
        view.assets.iter().map(|a| a.source_id.as_str()).collect()
    }

    #[test]
    fn should_order_assets_by_institution_not_fetch_order() {
        let view = build(
            &snapshot_of(json!([
                account("v", "Vanguard", "brokerage", "asset"),
                account("a", "Ally", "savings", "asset"),
                account("c", "Chase", "checking", "asset"),
            ])),
            None,
        );

        assert_eq!(asset_ids(&view), vec!["a", "c", "v"]);
    }

    #[test]
    fn should_order_liabilities_by_institution_not_fetch_order() {
        let view = build(
            &snapshot_of(json!([
                account("g", "Goldman Sachs", "credit_card", "liability"),
                account("a", "American Express", "credit_card", "liability"),
            ])),
            None,
        );

        let ids: Vec<&str> = view
            .liabilities
            .iter()
            .map(|a| a.source_id.as_str())
            .collect();
        assert_eq!(ids, vec!["a", "g"]);
    }

    #[test]
    fn should_order_institutions_ignoring_case() {
        let view = build(
            &snapshot_of(json!([
                account("c", "Chase", "checking", "asset"),
                account("a", "ally", "savings", "asset"),
            ])),
            None,
        );

        assert_eq!(asset_ids(&view), vec!["a", "c"]);
    }

    #[test]
    fn should_order_one_institution_by_account_type() {
        let view = build(
            &snapshot_of(json!([
                account("a_savings", "Chase", "savings", "asset"),
                account("z_checking", "Chase", "checking", "asset"),
            ])),
            None,
        );

        assert_eq!(asset_ids(&view), vec!["z_checking", "a_savings"]);
    }

    #[test]
    fn should_order_identical_accounts_by_source_id() {
        let view = build(
            &snapshot_of(json!([
                account("chase_2", "Chase", "checking", "asset"),
                account("chase_1", "Chase", "checking", "asset"),
            ])),
            None,
        );

        assert_eq!(asset_ids(&view), vec!["chase_1", "chase_2"]);
    }

    #[test]
    fn should_subtotal_ok_balances_per_group() {
        let view = build(
            &snapshot_of(json!([
                asset("chase", 1000.0),
                asset("ally", 500.0),
                liability("apple", 200.0),
            ])),
            None,
        );

        assert_eq!(
            (view.assets_subtotal, view.liabilities_subtotal),
            (1500.0, 200.0)
        );
    }

    #[test]
    fn should_report_positive_zero_subtotal_for_an_empty_group() {
        // f64's `Sum` starts from -0.0, which the UI's currency formatter
        // renders as "-$0.00" under an empty Liabilities group.
        let view = build(&snapshot_of(json!([asset("chase", 1000.0)])), None);

        assert!(view.liabilities_subtotal.is_sign_positive());
    }

    #[test]
    fn should_exclude_failed_account_from_subtotal_even_with_a_balance() {
        // A failed fetch can still carry a stale balance; it isn't a
        // trustworthy figure to add in (the TUI filters on status, not on
        // whether a balance happens to be present).
        let stale = json!({
            "account_key": "sha256:a",
            "source_id": "vanguard",
            "institution": "Vanguard",
            "category": "asset",
            "type": "brokerage",
            "balance": 999.0,
            "currency": "USD",
            "status": "error",
            "error_message": "boom",
        });

        let view = build(&snapshot_of(json!([asset("chase", 1000.0), stale])), None);

        assert_eq!(view.assets_subtotal, 1000.0);
    }

    #[test]
    fn should_exclude_unknown_status_account_from_subtotal() {
        let future = json!({
            "account_key": "sha256:a",
            "source_id": "vanguard",
            "institution": "Vanguard",
            "category": "asset",
            "type": "brokerage",
            "balance": 999.0,
            "currency": "USD",
            "status": "partially_settled",
        });

        let view = build(&snapshot_of(json!([asset("chase", 1000.0), future])), None);

        assert_eq!(view.assets_subtotal, 1000.0);
    }

    #[test]
    fn should_treat_missing_balance_as_zero_in_subtotal() {
        let no_balance = json!({
            "account_key": "sha256:a",
            "source_id": "ally",
            "institution": "Ally",
            "category": "asset",
            "type": "checking",
            "balance": null,
            "currency": "USD",
            "status": "ok",
        });

        let view = build(
            &snapshot_of(json!([asset("chase", 1000.0), no_balance])),
            None,
        );

        assert_eq!(view.assets_subtotal, 1000.0);
    }

    #[test]
    fn should_group_unrecognized_category_with_assets() {
        // D14: a category from a newer schema falls back to `Unknown`,
        // which `calculate_net_worth_from_records` counts on the asset
        // side. Grouping it anywhere else would leave the two groups
        // failing to reconcile with the net worth shown above them.
        let future = json!({
            "account_key": "sha256:a",
            "source_id": "equity_stake",
            "institution": "Carta",
            "category": "private_equity",
            "type": "rsu_stock",
            "balance": 500.0,
            "currency": "USD",
            "status": "ok",
        });

        let view = build(&snapshot_of(json!([future])), None);

        assert_eq!(view.assets.len(), 1);
    }

    #[test]
    fn should_map_every_account_field_from_the_record() {
        let view = build(&snapshot_of(json!([asset("chase", 1000.0)])), None);

        assert_eq!(
            view.assets[0],
            AccountView {
                source_id: "chase".into(),
                institution: "Chase".into(),
                label: None,
                account_type: "checking".into(),
                balance: Some(1000.0),
                currency: "USD".into(),
                status: StatusView::Ok,
                error_message: None,
            }
        );
    }

    #[test]
    fn should_carry_the_snapshot_timestamp() {
        let view = build(&snapshot_of(json!([asset("chase", 1000.0)])), None);

        assert_eq!(view.created_at, NOW);
    }

    #[test]
    fn should_report_error_status_for_failed_account() {
        let failed = json!({
            "account_key": "sha256:a",
            "source_id": "chase",
            "institution": "Chase",
            "category": "asset",
            "type": "checking",
            "balance": null,
            "currency": "USD",
            "status": "error",
            "error_message": "boom",
        });

        let view = build(&snapshot_of(json!([failed])), None);

        assert_eq!(view.assets[0].status, StatusView::Error);
    }

    #[test]
    fn should_map_unrecognized_status_to_unknown() {
        // D14's forward-compat fallback: a status written by a newer
        // schema than this build understands must not be collapsed into
        // `Error`, which would report a working source as broken.
        let future = json!({
            "account_key": "sha256:a",
            "source_id": "chase",
            "institution": "Chase",
            "category": "asset",
            "type": "checking",
            "balance": 1000.0,
            "currency": "USD",
            "status": "partially_settled",
        });

        let view = build(&snapshot_of(json!([future])), None);

        assert_eq!(view.assets[0].status, StatusView::Unknown);
    }

    #[test]
    fn should_carry_error_message_for_failed_account() {
        let failed = json!({
            "account_key": "sha256:a",
            "source_id": "vanguard",
            "institution": "Vanguard",
            "category": "asset",
            "type": "brokerage",
            "balance": null,
            "currency": "USD",
            "status": "error",
            "error_message": "no statement found in watch_dir",
        });

        let view = build(&snapshot_of(json!([failed])), None);

        assert_eq!(
            view.assets[0].error_message.as_deref(),
            Some("no statement found in watch_dir")
        );
    }

    #[test]
    fn should_compute_delta_against_previous_snapshot() {
        let view = build(
            &snapshot_of(json!([asset("chase", 1100.0)])),
            Some(&snapshot_at(EARLIER, json!([asset("chase", 1000.0)]))),
        );

        assert_eq!(
            view.delta,
            Some(DeltaView {
                amount: 100.0,
                since: EARLIER.to_string(),
            })
        );
    }

    #[test]
    fn should_omit_delta_when_no_previous_snapshot() {
        let view = build(&snapshot_of(json!([asset("chase", 1000.0)])), None);

        assert_eq!(view.delta, None);
    }

    #[test]
    fn should_omit_delta_when_previous_net_worth_is_unavailable() {
        let view = build(
            &snapshot_of(json!([asset("chase", 1100.0)])),
            Some(&snapshot_at(EARLIER, json!([]))),
        );

        assert_eq!(view.delta, None);
    }
}
