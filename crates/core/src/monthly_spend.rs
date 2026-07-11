//! Credit card spend trend (spec §13.4, D39) — charts the total balance
//! of `Category::Liability` accounts (today: Chase credit card + Apple
//! Card) over time, as a proxy for "monthly spend," under the explicit
//! assumption that these cards are paid off each cycle so the
//! statement balance roughly equals that period's spend. Real
//! transaction-level spend was explored and explicitly parked (D38) —
//! this is a much narrower, cheaper proxy over data already extracted
//! every run, not a reopening of that decision.
//!
//! Pure, no I/O — mirrors `emergency_fund.rs`'s shape exactly, just
//! with an inverted threshold direction ("higher is worse," not
//! "lower is worse"), hence the separate `band_for_spend` rather than
//! reusing `emergency_fund::band_for`.

use serde::{Deserialize, Serialize};
use time::format_description::well_known::Rfc3339;
use time::OffsetDateTime;

use crate::snapshot::{AccountRecord, Category, Snapshot, Status};
use crate::threshold_band::ThresholdBand;

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct MonthlySpendThresholds {
    pub yellow_at_or_above: f64,
    pub red_at_or_above: f64,
}

impl Default for MonthlySpendThresholds {
    /// User-given starting point (D39) — unlike
    /// `EmergencyFundThresholds::target_monthly_expenses`, both figures
    /// already have a sensible default, so no first-run prompt is
    /// needed here.
    fn default() -> Self {
        Self {
            yellow_at_or_above: 8000.0,
            red_at_or_above: 11000.0,
        }
    }
}

/// How many recent snapshots `extract_spend_series` callers should
/// request from `load_recent_snapshots` — runs aren't evenly spaced
/// (no scheduled runs until v0.4), so this reflects "recent runs," not
/// calendar months.
pub const HISTORY_LIMIT: usize = 18;

/// "Higher is worse" — the inverse comparison direction from
/// `emergency_fund::band_for`'s "lower is worse."
pub fn band_for_spend(total: f64, thresholds: &MonthlySpendThresholds) -> ThresholdBand {
    if total >= thresholds.red_at_or_above {
        ThresholdBand::Red
    } else if total >= thresholds.yellow_at_or_above {
        ThresholdBand::Yellow
    } else {
        ThresholdBand::Green
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum CurrentPeriodSpend {
    Computed { total: f64, band: ThresholdBand },
    /// No `Category::Liability` account with `Status::Ok` exists in
    /// this snapshot — distinct from "those accounts exist and sum to
    /// $0" (same "never a bare misleading number" instinct as
    /// `EmergencyFundStatus::NoCashAccountData`, D36).
    NoLiabilityAccountData,
}

/// Sums every `Category::Liability` account's balance (only those
/// whose fetch succeeded this run, `Status::Ok`) for the current
/// snapshot.
pub fn calculate_current_period_spend(
    records: &[AccountRecord],
    thresholds: &MonthlySpendThresholds,
) -> CurrentPeriodSpend {
    let qualifying: Vec<f64> = records
        .iter()
        .filter(|record| record.category() == Category::Liability && record.status() == Status::Ok)
        .filter_map(|record| record.balance())
        .collect();

    if qualifying.is_empty() {
        return CurrentPeriodSpend::NoLiabilityAccountData;
    }

    let total: f64 = qualifying.iter().sum();
    let band = band_for_spend(total, thresholds);

    CurrentPeriodSpend::Computed { total, band }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpendPoint {
    pub total: f64,
    /// `None` if this snapshot's `created_at` failed to parse — the
    /// point is still kept (never silently dropped, §9.1's instinct),
    /// just excluded from the plotted line by the caller.
    pub timestamp: Option<OffsetDateTime>,
}

/// Turns `load_recent_snapshots`'s newest-first history into an
/// oldest-first time series (a chart's x-axis reads left-to-right
/// chronologically), summing each snapshot's `Category::Liability`/
/// `Status::Ok` balances the same way `calculate_current_period_spend`
/// does for a single snapshot.
pub fn extract_spend_series(snapshots: &[Snapshot]) -> Vec<SpendPoint> {
    snapshots
        .iter()
        .rev()
        .map(|snapshot| {
            let total: f64 = snapshot
                .accounts
                .iter()
                .filter(|record| {
                    record.category() == Category::Liability && record.status() == Status::Ok
                })
                .filter_map(|record| record.balance())
                .sum();
            let timestamp = OffsetDateTime::parse(&snapshot.created_at, &Rfc3339).ok();
            SpendPoint { total, timestamp }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_thresholds() -> MonthlySpendThresholds {
        MonthlySpendThresholds {
            yellow_at_or_above: 8000.0,
            red_at_or_above: 11000.0,
        }
    }

    fn account(category: Category, balance: Option<f64>, status: Status) -> AccountRecord {
        AccountRecord {
            account_key: "sha256:fake".into(),
            source_id: "fake_source".into(),
            institution: "Fake Bank".into(),
            category,
            account_type: "credit_card".into(),
            balance,
            currency: "USD".into(),
            status,
            error_message: None,
            holdings: None,
        }
    }

    fn snapshot(created_at: &str, accounts: Vec<AccountRecord>) -> Snapshot {
        Snapshot {
            schema_version: 1,
            snapshot_id: "test".into(),
            created_at: created_at.into(),
            accounts,
        }
    }

    #[test]
    fn default_thresholds_match_the_user_given_starting_point() {
        let thresholds = MonthlySpendThresholds::default();
        assert_eq!(thresholds.yellow_at_or_above, 8000.0);
        assert_eq!(thresholds.red_at_or_above, 11000.0);
    }

    #[test]
    fn band_for_spend_below_yellow_is_green() {
        let thresholds = configured_thresholds();
        assert_eq!(band_for_spend(7999.99, &thresholds), ThresholdBand::Green);
    }

    #[test]
    fn band_for_spend_exactly_at_yellow_is_yellow() {
        let thresholds = configured_thresholds();
        assert_eq!(band_for_spend(8000.0, &thresholds), ThresholdBand::Yellow);
    }

    #[test]
    fn band_for_spend_between_yellow_and_red_is_yellow() {
        let thresholds = configured_thresholds();
        assert_eq!(band_for_spend(9500.0, &thresholds), ThresholdBand::Yellow);
    }

    #[test]
    fn band_for_spend_exactly_at_red_is_red() {
        let thresholds = configured_thresholds();
        assert_eq!(band_for_spend(11000.0, &thresholds), ThresholdBand::Red);
    }

    #[test]
    fn band_for_spend_above_red_is_red() {
        let thresholds = configured_thresholds();
        assert_eq!(band_for_spend(15000.0, &thresholds), ThresholdBand::Red);
    }

    #[test]
    fn calculate_current_period_spend_sums_only_liability_ok_accounts() {
        let records = vec![
            account(Category::Liability, Some(1000.0), Status::Ok),
            account(Category::Liability, Some(2000.0), Status::Ok),
        ];
        let status = calculate_current_period_spend(&records, &configured_thresholds());

        match status {
            CurrentPeriodSpend::Computed { total, .. } => assert_eq!(total, 3000.0),
            other => panic!("expected Computed, got {other:?}"),
        }
    }

    #[test]
    fn calculate_current_period_spend_excludes_an_errored_liability_account() {
        let records = vec![
            account(Category::Liability, Some(1000.0), Status::Ok),
            account(Category::Liability, Some(99999.0), Status::Error),
        ];
        let status = calculate_current_period_spend(&records, &configured_thresholds());

        match status {
            CurrentPeriodSpend::Computed { total, .. } => assert_eq!(total, 1000.0),
            other => panic!("expected Computed, got {other:?}"),
        }
    }

    #[test]
    fn calculate_current_period_spend_ignores_asset_accounts() {
        let records = vec![
            account(Category::Asset, Some(50000.0), Status::Ok),
            account(Category::Liability, Some(1000.0), Status::Ok),
        ];
        let status = calculate_current_period_spend(&records, &configured_thresholds());

        match status {
            CurrentPeriodSpend::Computed { total, .. } => assert_eq!(total, 1000.0),
            other => panic!("expected Computed, got {other:?}"),
        }
    }

    #[test]
    fn no_liability_accounts_returns_no_liability_account_data() {
        let records = vec![account(Category::Asset, Some(50000.0), Status::Ok)];
        let status = calculate_current_period_spend(&records, &configured_thresholds());

        assert_eq!(status, CurrentPeriodSpend::NoLiabilityAccountData);
    }

    #[test]
    fn zero_balance_liability_account_is_still_computed_not_conflated_with_no_data() {
        let records = vec![account(Category::Liability, Some(0.0), Status::Ok)];
        let status = calculate_current_period_spend(&records, &configured_thresholds());

        assert_eq!(
            status,
            CurrentPeriodSpend::Computed {
                total: 0.0,
                band: ThresholdBand::Green,
            }
        );
    }

    #[test]
    fn extract_spend_series_reorders_newest_first_input_to_oldest_first_output() {
        let snapshots = vec![
            snapshot(
                "2026-03-01T00:00:00Z",
                vec![account(Category::Liability, Some(300.0), Status::Ok)],
            ),
            snapshot(
                "2026-02-01T00:00:00Z",
                vec![account(Category::Liability, Some(200.0), Status::Ok)],
            ),
            snapshot(
                "2026-01-01T00:00:00Z",
                vec![account(Category::Liability, Some(100.0), Status::Ok)],
            ),
        ];
        let series = extract_spend_series(&snapshots);

        assert_eq!(series[0].total, 100.0);
        assert_eq!(series[1].total, 200.0);
        assert_eq!(series[2].total, 300.0);
    }

    #[test]
    fn extract_spend_series_sums_only_liability_ok_balances_per_snapshot() {
        let snapshots = vec![snapshot(
            "2026-01-01T00:00:00Z",
            vec![
                account(Category::Liability, Some(1000.0), Status::Ok),
                account(Category::Asset, Some(50000.0), Status::Ok),
                account(Category::Liability, Some(99999.0), Status::Error),
            ],
        )];
        let series = extract_spend_series(&snapshots);

        assert_eq!(series.len(), 1);
        assert_eq!(series[0].total, 1000.0);
    }

    #[test]
    fn extract_spend_series_on_empty_history_returns_an_empty_series() {
        let series = extract_spend_series(&[]);
        assert!(series.is_empty());
    }

    #[test]
    fn extract_spend_series_parses_rfc3339_created_at_into_a_timestamp() {
        let snapshots = vec![snapshot(
            "2026-06-30T09:15:00-07:00",
            vec![account(Category::Liability, Some(100.0), Status::Ok)],
        )];
        let series = extract_spend_series(&snapshots);

        assert!(series[0].timestamp.is_some());
    }

    #[test]
    fn extract_spend_series_keeps_a_point_with_an_unparseable_created_at_but_leaves_its_timestamp_none(
    ) {
        let snapshots = vec![snapshot(
            "not a real timestamp",
            vec![account(Category::Liability, Some(100.0), Status::Ok)],
        )];
        let series = extract_spend_series(&snapshots);

        assert_eq!(series.len(), 1);
        assert_eq!(series[0].total, 100.0);
        assert!(series[0].timestamp.is_none());
    }

    #[test]
    fn thresholds_serialize_and_deserialize_correctly() {
        let thresholds = configured_thresholds();
        let json = serde_json::to_string(&thresholds).unwrap();
        let round_tripped: MonthlySpendThresholds = serde_json::from_str(&json).unwrap();

        assert_eq!(thresholds, round_tripped);
    }
}
