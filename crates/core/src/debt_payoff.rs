//! High-interest debt payoff priority (spec §13.1, D41) — a generic
//! financial-planning best practice ("pay down high-interest debt
//! before investing further"), not personalized to any specific plan.
//! Applies to any `Category::Liability` account, not just credit
//! cards — a mortgage or student loan works the same way with no code
//! change.
//!
//! Pure, no I/O — mirrors `emergency_fund.rs`'s shape. Deliberately
//! does **not** reuse `ThresholdBand`: this is a binary flag (meets/
//! exceeds the threshold or not), not a three-color gradient, so
//! forcing it into Red/Yellow/Green would be a bad fit.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::snapshot::{AccountRecord, Category, Status};

/// Keyed by `SourceConfig.id` (the stable, user-chosen id visible on
/// the Sources screen — the only account identifier a person could
/// plausibly hand-type into `rules.yaml`, unlike the salted
/// `account_key`). Values are a plain percentage number (`24.99` means
/// 24.99% APR), not a fraction.
pub type DebtInterestRates = BTreeMap<String, f64>;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DebtPayoffConfig {
    pub high_interest_at_or_above: f64,
    #[serde(default)]
    pub interest_rates: DebtInterestRates,
}

impl Default for DebtPayoffConfig {
    /// `high_interest_at_or_above: 7.0` — the commonly-cited generic
    /// benchmark (guaranteed debt payoff beats expected market return
    /// above roughly this range); `interest_rates` starts empty,
    /// driving `DebtPayoffStatus::NoRatesConfigured` until at least
    /// one rate is hand-entered.
    fn default() -> Self {
        Self {
            high_interest_at_or_above: 7.0,
            interest_rates: DebtInterestRates::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct FlaggedDebt {
    pub source_id: String,
    pub institution: String,
    pub account_type: String,
    pub balance: f64,
    pub interest_rate: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DebtPayoffStatus {
    /// Sorted by `interest_rate` descending — highest priority first.
    Flagged(Vec<FlaggedDebt>),
    NoHighInterestDebt,
    /// `interest_rates` is empty — distinct from "checked and nothing
    /// qualifies," a real "you haven't set this up yet" state.
    NoRatesConfigured,
}

/// Flags every `Category::Liability`/`Status::Ok` account whose
/// configured rate meets or exceeds the threshold. An account with no
/// configured rate, the wrong category, a non-`Ok` status, or (in
/// principle) a missing balance is silently excluded, never flagged.
pub fn evaluate_debt_payoff_priority(
    records: &[AccountRecord],
    config: &DebtPayoffConfig,
) -> DebtPayoffStatus {
    if config.interest_rates.is_empty() {
        return DebtPayoffStatus::NoRatesConfigured;
    }

    let mut flagged: Vec<FlaggedDebt> = records
        .iter()
        .filter(|record| record.category() == Category::Liability && record.status() == Status::Ok)
        .filter_map(|record| {
            let rate = *config.interest_rates.get(record.source_id())?;
            if rate < config.high_interest_at_or_above {
                return None;
            }
            let balance = record.balance()?;
            Some(FlaggedDebt {
                source_id: record.source_id().to_string(),
                institution: record.institution().to_string(),
                account_type: record.account_type().to_string(),
                balance,
                interest_rate: rate,
            })
        })
        .collect();

    flagged.sort_by(|a, b| b.interest_rate.partial_cmp(&a.interest_rate).unwrap());

    if flagged.is_empty() {
        DebtPayoffStatus::NoHighInterestDebt
    } else {
        DebtPayoffStatus::Flagged(flagged)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configured_config() -> DebtPayoffConfig {
        let mut interest_rates = DebtInterestRates::new();
        interest_rates.insert("chase_credit_card".to_string(), 24.99);
        interest_rates.insert("apple_card".to_string(), 3.99);
        DebtPayoffConfig {
            high_interest_at_or_above: 7.0,
            interest_rates,
        }
    }

    fn liability_account(source_id: &str, balance: Option<f64>, status: Status) -> AccountRecord {
        AccountRecord {
            account_key: "sha256:fake".into(),
            source_id: source_id.into(),
            institution: "Fake Bank".into(),
            category: Category::Liability,
            account_type: "credit_card".into(),
            balance,
            currency: "USD".into(),
            status,
            error_message: None,
            holdings: None,
        }
    }

    fn asset_account(source_id: &str, balance: Option<f64>) -> AccountRecord {
        AccountRecord {
            account_key: "sha256:fake".into(),
            source_id: source_id.into(),
            institution: "Fake Bank".into(),
            category: Category::Asset,
            account_type: "checking".into(),
            balance,
            currency: "USD".into(),
            status: Status::Ok,
            error_message: None,
            holdings: None,
        }
    }

    #[test]
    fn default_config_uses_seven_percent_threshold_and_empty_rates() {
        let config = DebtPayoffConfig::default();
        assert_eq!(config.high_interest_at_or_above, 7.0);
        assert!(config.interest_rates.is_empty());
    }

    #[test]
    fn no_rates_configured_returns_no_rates_configured_state() {
        let records = vec![liability_account("chase_credit_card", Some(1000.0), Status::Ok)];
        let status = evaluate_debt_payoff_priority(&records, &DebtPayoffConfig::default());

        assert_eq!(status, DebtPayoffStatus::NoRatesConfigured);
    }

    #[test]
    fn a_liability_below_threshold_is_not_flagged() {
        let records = vec![liability_account("apple_card", Some(500.0), Status::Ok)];
        let status = evaluate_debt_payoff_priority(&records, &configured_config());

        assert_eq!(status, DebtPayoffStatus::NoHighInterestDebt);
    }

    #[test]
    fn a_liability_at_or_above_threshold_is_flagged() {
        let records = vec![liability_account("chase_credit_card", Some(4231.0), Status::Ok)];
        let status = evaluate_debt_payoff_priority(&records, &configured_config());

        match status {
            DebtPayoffStatus::Flagged(debts) => {
                assert_eq!(debts.len(), 1);
                assert_eq!(debts[0].source_id, "chase_credit_card");
                assert_eq!(debts[0].balance, 4231.0);
                assert_eq!(debts[0].interest_rate, 24.99);
            }
            other => panic!("expected Flagged, got {other:?}"),
        }
    }

    #[test]
    fn an_asset_account_is_never_flagged_even_with_a_configured_rate() {
        let records = vec![asset_account("chase_credit_card", Some(4231.0))];
        let status = evaluate_debt_payoff_priority(&records, &configured_config());

        assert_eq!(status, DebtPayoffStatus::NoHighInterestDebt);
    }

    #[test]
    fn an_errored_liability_account_is_excluded_even_with_a_configured_rate() {
        let records = vec![liability_account(
            "chase_credit_card",
            Some(4231.0),
            Status::Error,
        )];
        let status = evaluate_debt_payoff_priority(&records, &configured_config());

        assert_eq!(status, DebtPayoffStatus::NoHighInterestDebt);
    }

    #[test]
    fn a_liability_with_no_configured_rate_is_not_flagged() {
        let records = vec![liability_account("some_other_card", Some(4231.0), Status::Ok)];
        let status = evaluate_debt_payoff_priority(&records, &configured_config());

        assert_eq!(status, DebtPayoffStatus::NoHighInterestDebt);
    }

    #[test]
    fn multiple_flagged_debts_are_sorted_by_rate_descending() {
        let mut interest_rates = DebtInterestRates::new();
        interest_rates.insert("card_a".to_string(), 15.0);
        interest_rates.insert("card_b".to_string(), 25.0);
        interest_rates.insert("card_c".to_string(), 20.0);
        let config = DebtPayoffConfig {
            high_interest_at_or_above: 7.0,
            interest_rates,
        };
        let records = vec![
            liability_account("card_a", Some(100.0), Status::Ok),
            liability_account("card_b", Some(100.0), Status::Ok),
            liability_account("card_c", Some(100.0), Status::Ok),
        ];
        let status = evaluate_debt_payoff_priority(&records, &config);

        match status {
            DebtPayoffStatus::Flagged(debts) => {
                let rates: Vec<f64> = debts.iter().map(|d| d.interest_rate).collect();
                assert_eq!(rates, vec![25.0, 20.0, 15.0]);
            }
            other => panic!("expected Flagged, got {other:?}"),
        }
    }

    #[test]
    fn all_configured_rates_below_threshold_returns_no_high_interest_debt() {
        let mut interest_rates = DebtInterestRates::new();
        interest_rates.insert("apple_card".to_string(), 3.99);
        let config = DebtPayoffConfig {
            high_interest_at_or_above: 7.0,
            interest_rates,
        };
        let records = vec![liability_account("apple_card", Some(500.0), Status::Ok)];
        let status = evaluate_debt_payoff_priority(&records, &config);

        assert_eq!(status, DebtPayoffStatus::NoHighInterestDebt);
    }

    #[test]
    fn config_serializes_and_deserializes_correctly() {
        let config = configured_config();
        let json = serde_json::to_string(&config).unwrap();
        let round_tripped: DebtPayoffConfig = serde_json::from_str(&json).unwrap();

        assert_eq!(config, round_tripped);
    }
}
