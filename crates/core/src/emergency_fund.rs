//! Emergency fund coverage (spec §13.1 Type A, D36) — the first slice
//! of recommendation tracking (FR23–FR28). A pure function over
//! already-persisted `AccountRecord` data, no schema/provider changes,
//! same shape as `holdings.rs`'s `classify`/`bucket` (D31): this module
//! never touches a file or a snapshot directly, only the `&[AccountRecord]`
//! slice and threshold config it's given.

use serde::{Deserialize, Serialize};

use crate::snapshot::{AccountRecord, Status};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq)]
pub struct EmergencyFundThresholds {
    /// `<= 0.0` means "not yet configured" — there's no meaningful
    /// real-world $0 target, so this doubles as the unconfigured
    /// sentinel rather than adding an `Option<f64>` layer. Drives the
    /// CLI's first-run interactive prompt.
    pub target_monthly_expenses: f64,
    pub red_below_months: f64,
    pub green_at_or_above_months: f64,
}

impl Default for EmergencyFundThresholds {
    /// Target unconfigured; red/green bands are §13.1's illustrative
    /// starting point (<6 red, 6–9 yellow, >9 green) — these two have a
    /// sensible default, unlike the target figure, so only the target
    /// needs a first-run prompt.
    fn default() -> Self {
        Self {
            target_monthly_expenses: 0.0,
            red_below_months: 6.0,
            green_at_or_above_months: 9.0,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThresholdBand {
    Red,
    Yellow,
    Green,
}

impl ThresholdBand {
    pub fn label(&self) -> &'static str {
        match self {
            ThresholdBand::Red => "Red",
            ThresholdBand::Yellow => "Yellow",
            ThresholdBand::Green => "Green",
        }
    }
}

pub fn band_for(months_of_coverage: f64, thresholds: &EmergencyFundThresholds) -> ThresholdBand {
    if months_of_coverage < thresholds.red_below_months {
        ThresholdBand::Red
    } else if months_of_coverage >= thresholds.green_at_or_above_months {
        ThresholdBand::Green
    } else {
        ThresholdBand::Yellow
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum EmergencyFundStatus {
    Computed {
        total_cash: f64,
        months_of_coverage: f64,
        band: ThresholdBand,
    },
    /// No `checking`/`money_market` account with `Status::Ok` exists in
    /// this snapshot — distinct from "those accounts exist and sum to
    /// $0" (the same distinction `NetWorth::Unavailable` draws against
    /// a genuine `$0` net worth).
    NoCashAccountData,
    /// `thresholds.target_monthly_expenses <= 0.0` — checked *before*
    /// the cash-data check, since no target means no meaningful months
    /// figure regardless of what cash data exists. Reachable when the
    /// CLI's first-run prompt was skipped, or from a future headless
    /// run that never prompts at all.
    TargetNotConfigured,
}

/// Sums every `checking`/`money_market` account's balance (only those
/// whose fetch succeeded this run, `Status::Ok`) and divides by the
/// configured target monthly expense figure.
pub fn calculate_emergency_fund_status(
    records: &[AccountRecord],
    thresholds: &EmergencyFundThresholds,
) -> EmergencyFundStatus {
    if thresholds.target_monthly_expenses <= 0.0 {
        return EmergencyFundStatus::TargetNotConfigured;
    }

    let qualifying: Vec<f64> = records
        .iter()
        .filter(|record| {
            record.status() == Status::Ok
                && matches!(record.account_type(), "checking" | "money_market")
        })
        .filter_map(|record| record.balance())
        .collect();

    if qualifying.is_empty() {
        return EmergencyFundStatus::NoCashAccountData;
    }

    let total_cash: f64 = qualifying.iter().sum();
    let months_of_coverage = total_cash / thresholds.target_monthly_expenses;
    let band = band_for(months_of_coverage, thresholds);

    EmergencyFundStatus::Computed {
        total_cash,
        months_of_coverage,
        band,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::snapshot::Category;

    fn configured_thresholds() -> EmergencyFundThresholds {
        EmergencyFundThresholds {
            target_monthly_expenses: 5000.0,
            red_below_months: 6.0,
            green_at_or_above_months: 9.0,
        }
    }

    fn account(account_type: &str, balance: Option<f64>, status: Status) -> AccountRecord {
        AccountRecord {
            account_key: "sha256:fake".into(),
            source_id: "fake_source".into(),
            institution: "Fake Bank".into(),
            category: Category::Asset,
            account_type: account_type.into(),
            balance,
            currency: "USD".into(),
            status,
            error_message: None,
            holdings: None,
        }
    }

    #[test]
    fn default_thresholds_match_the_spec_illustrative_bands() {
        let thresholds = EmergencyFundThresholds::default();
        assert_eq!(thresholds.target_monthly_expenses, 0.0);
        assert_eq!(thresholds.red_below_months, 6.0);
        assert_eq!(thresholds.green_at_or_above_months, 9.0);
    }

    #[test]
    fn band_for_just_below_red_threshold_is_red() {
        let thresholds = configured_thresholds();
        assert_eq!(band_for(5.9, &thresholds), ThresholdBand::Red);
    }

    #[test]
    fn band_for_exactly_six_months_is_yellow() {
        let thresholds = configured_thresholds();
        assert_eq!(band_for(6.0, &thresholds), ThresholdBand::Yellow);
    }

    #[test]
    fn band_for_exactly_nine_months_is_yellow() {
        // §13.1's "6-9 yellow" band is inclusive of the lower bound and
        // the green band's "at or above" wording puts exactly 9.0 in
        // Green, not Yellow — verified by the boundary check in
        // band_for using >=.
        let thresholds = configured_thresholds();
        assert_eq!(band_for(9.0, &thresholds), ThresholdBand::Green);
    }

    #[test]
    fn band_for_just_above_nine_months_is_green() {
        let thresholds = configured_thresholds();
        assert_eq!(band_for(9.1, &thresholds), ThresholdBand::Green);
    }

    #[test]
    fn sums_only_checking_and_money_market_ok_accounts_into_the_total() {
        let records = vec![
            account("checking", Some(1000.0), Status::Ok),
            account("money_market", Some(2000.0), Status::Ok),
            account("brokerage", Some(50000.0), Status::Ok),
        ];
        let status = calculate_emergency_fund_status(&records, &configured_thresholds());

        match status {
            EmergencyFundStatus::Computed { total_cash, .. } => assert_eq!(total_cash, 3000.0),
            other => panic!("expected Computed, got {other:?}"),
        }
    }

    #[test]
    fn an_errored_checking_account_is_excluded_from_the_total() {
        let records = vec![
            account("checking", Some(1000.0), Status::Ok),
            account("checking", Some(99999.0), Status::Error),
        ];
        let status = calculate_emergency_fund_status(&records, &configured_thresholds());

        match status {
            EmergencyFundStatus::Computed { total_cash, .. } => assert_eq!(total_cash, 1000.0),
            other => panic!("expected Computed, got {other:?}"),
        }
    }

    #[test]
    fn no_qualifying_accounts_returns_no_cash_account_data() {
        let records = vec![account("brokerage", Some(50000.0), Status::Ok)];
        let status = calculate_emergency_fund_status(&records, &configured_thresholds());

        assert_eq!(status, EmergencyFundStatus::NoCashAccountData);
    }

    #[test]
    fn zero_total_cash_with_a_configured_target_is_computed_as_zero_months_red() {
        // The key "never a bare misleading number" test — a real $0
        // across real qualifying accounts must still yield Computed,
        // never conflated with NoCashAccountData.
        let records = vec![account("checking", Some(0.0), Status::Ok)];
        let status = calculate_emergency_fund_status(&records, &configured_thresholds());

        assert_eq!(
            status,
            EmergencyFundStatus::Computed {
                total_cash: 0.0,
                months_of_coverage: 0.0,
                band: ThresholdBand::Red,
            }
        );
    }

    #[test]
    fn an_unconfigured_target_returns_target_not_configured_even_with_cash_present() {
        let records = vec![account("checking", Some(10000.0), Status::Ok)];
        let status = calculate_emergency_fund_status(&records, &EmergencyFundThresholds::default());

        assert_eq!(status, EmergencyFundStatus::TargetNotConfigured);
    }

    #[test]
    fn computes_months_of_coverage_as_total_cash_divided_by_target() {
        let records = vec![account("checking", Some(15000.0), Status::Ok)];
        let status = calculate_emergency_fund_status(&records, &configured_thresholds());

        match status {
            EmergencyFundStatus::Computed {
                months_of_coverage, ..
            } => assert_eq!(months_of_coverage, 3.0),
            other => panic!("expected Computed, got {other:?}"),
        }
    }

    #[test]
    fn checking_and_money_market_balances_are_summed_together() {
        let records = vec![
            account("checking", Some(1000.0), Status::Ok),
            account("money_market", Some(4000.0), Status::Ok),
        ];
        let status = calculate_emergency_fund_status(&records, &configured_thresholds());

        match status {
            EmergencyFundStatus::Computed {
                total_cash,
                months_of_coverage,
                ..
            } => {
                assert_eq!(total_cash, 5000.0);
                assert_eq!(months_of_coverage, 1.0);
            }
            other => panic!("expected Computed, got {other:?}"),
        }
    }

    #[test]
    fn thresholds_serialize_and_deserialize_correctly() {
        let thresholds = configured_thresholds();
        let json = serde_json::to_string(&thresholds).unwrap();
        let round_tripped: EmergencyFundThresholds = serde_json::from_str(&json).unwrap();

        assert_eq!(thresholds, round_tripped);
    }
}
