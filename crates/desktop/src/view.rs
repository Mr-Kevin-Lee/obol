//! The dashboard's view model (spec §16 v0.3a, D43) — the shape the
//! webview renders, assembled from a `Snapshot`.
//!
//! This exists because `obol-core`'s own types can't cross the Tauri
//! boundary as they are: `Snapshot`/`AccountRecord` do derive `Serialize`
//! and carry no PII by design (§11.1), but [`NetWorth`] and [`AssetClass`]
//! derive neither, and both the net worth figure and the holdings buckets
//! have to be *computed* rather than read off the snapshot.
//!
//! What it deliberately is **not** is a second home for domain logic
//! (D43's "no logic rewrite"): the sign rule for net worth stays in
//! `calculate_net_worth_from_records` where D11 put it, and asset-class
//! classification stays in `holdings::bucket`. This module only groups,
//! maps and computes percentages.

use obol_core::{
    bucket, calculate_net_worth_from_records, AccountRecord, Category, Holding, NetWorth, Snapshot,
    Status,
};
use serde::Serialize;

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
}

/// Internally tagged so the frontend branches on `kind` rather than on
/// a null check. §9.1 forbids rendering an all-failed run as `$0`, which
/// is indistinguishable from a genuine zero, so "no value" is a distinct
/// variant carrying why rather than an absent number.
#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum NetWorthView {
    Computed { value: f64 },
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
    pub account_type: String,
    pub balance: Option<f64>,
    pub currency: String,
    pub status: StatusView,
    /// Present only for a failed source; FR18 requires its panel show
    /// this rather than a blank.
    pub error_message: Option<String>,
    /// `None` for accounts that carry no positions (FR22 applies only to
    /// holdings-bearing accounts).
    pub holdings: Option<Vec<HoldingBucketView>>,
}

/// One proportional bar in an account's holdings breakdown (FR22).
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct HoldingBucketView {
    pub label: String,
    pub value: f64,
    pub percentage: f64,
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
    }
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
        account_type: record.account_type().to_string(),
        balance: record.balance(),
        currency: record.currency().to_string(),
        status: match record.status() {
            Status::Ok => StatusView::Ok,
            Status::Error => StatusView::Error,
            Status::Unknown => StatusView::Unknown,
        },
        error_message: record.error_message().map(str::to_string),
        holdings: record.holdings().map(holding_buckets),
    }
}

fn holding_buckets(holdings: &[Holding]) -> Vec<HoldingBucketView> {
    let buckets = bucket(holdings);
    let total: f64 = buckets.iter().map(|(_, value)| value).sum();

    buckets
        .into_iter()
        .map(|(class, value)| HoldingBucketView {
            label: class.label().to_string(),
            value,
            // Positions summing to nothing have no proportions to show;
            // dividing anyway would hand the frontend NaN bar widths.
            percentage: if total == 0.0 {
                0.0
            } else {
                value / total * 100.0
            },
        })
        .collect()
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
        amount: current - before,
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
                account_type: "checking".into(),
                balance: Some(1000.0),
                currency: "USD".into(),
                status: StatusView::Ok,
                error_message: None,
                holdings: None,
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
    fn should_bucket_holdings_by_asset_class_with_percentages() {
        let holder = json!({
            "account_key": "sha256:a",
            "source_id": "vanguard",
            "institution": "Vanguard",
            "category": "asset",
            "type": "brokerage",
            "balance": 1000.0,
            "currency": "USD",
            "status": "ok",
            "holdings": [
                { "symbol": "VMFXX", "description": "VANGUARD FEDERAL MONEY MARKET", "value": 250.0 },
                { "symbol": "AAPL", "description": "APPLE INC", "value": 750.0 },
            ],
        });

        let view = build(&snapshot_of(json!([holder])), None);

        assert_eq!(
            view.assets[0].holdings.as_deref(),
            Some(
                [
                    HoldingBucketView {
                        label: "Cash".into(),
                        value: 250.0,
                        percentage: 25.0,
                    },
                    HoldingBucketView {
                        label: "Individual Stock".into(),
                        value: 750.0,
                        percentage: 75.0,
                    },
                ]
                .as_slice()
            )
        );
    }

    #[test]
    fn should_omit_holdings_when_account_has_none() {
        let view = build(&snapshot_of(json!([asset("chase", 1000.0)])), None);

        assert_eq!(view.assets[0].holdings, None);
    }

    #[test]
    fn should_report_zero_percentage_when_holdings_total_is_zero() {
        let closed = json!({
            "account_key": "sha256:a",
            "source_id": "vanguard",
            "institution": "Vanguard",
            "category": "asset",
            "type": "brokerage",
            "balance": 0.0,
            "currency": "USD",
            "status": "ok",
            "holdings": [
                { "symbol": "AAPL", "description": "APPLE INC", "value": 0.0 },
            ],
        });

        let view = build(&snapshot_of(json!([closed])), None);

        assert_eq!(view.assets[0].holdings.as_ref().unwrap()[0].percentage, 0.0);
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
