use serde::{Deserialize, Serialize};

/// Common interface over asset and liability holdings (decision D11, spec §11).
///
/// `Asset` and `Liability` implement this rather than being distinguished by
/// a `category` field branched on throughout net worth calc and dashboard
/// rendering.
pub trait Account: Send + Sync + std::fmt::Debug {
    fn account_key(&self) -> &str;
    fn institution(&self) -> &str;
    fn balance(&self) -> Option<f64>;
    fn status(&self) -> &AccountStatus;

    /// Signed contribution to net worth — positive for assets, negative
    /// for liabilities. The one place this sign logic lives.
    fn net_worth_contribution(&self) -> f64;

    /// This account's individual positions, if it has any (spec D31) —
    /// `None` for every account type except a holdings-bearing one
    /// (currently only Vanguard Brokerage, via `StatementImportProvider`).
    /// Default-implemented so no existing `Account` impl needs to change
    /// just because this method exists.
    fn holdings(&self) -> Option<&[Holding]> {
        None
    }
}

/// One position within a holdings-bearing account (spec D31) — e.g. a
/// single line from a brokerage statement's Holdings/Positions table.
/// Deliberately carries no asset-class label itself (cash vs. ETF vs.
/// individual stock) — that classification is a pure, independently
/// testable function over already-extracted `description` text
/// (`crate::holdings::classify`), not baked into parsing or persistence,
/// so reclassifying later never requires a new statement parse or a
/// schema migration.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Holding {
    pub symbol: String,
    pub description: String,
    pub value: f64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AccountStatus {
    Ok,
    Error { message: String },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Asset {
    pub account_key: String,
    pub institution: String,
    pub r#type: String,
    pub balance: Option<f64>,
    pub status: AccountStatus,
    pub holdings: Option<Vec<Holding>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Liability {
    pub account_key: String,
    pub institution: String,
    pub r#type: String,
    pub balance: Option<f64>,
    pub status: AccountStatus,
}

impl Account for Asset {
    fn account_key(&self) -> &str {
        &self.account_key
    }

    fn institution(&self) -> &str {
        &self.institution
    }

    fn balance(&self) -> Option<f64> {
        self.balance
    }

    fn status(&self) -> &AccountStatus {
        &self.status
    }

    fn net_worth_contribution(&self) -> f64 {
        self.balance.unwrap_or(0.0)
    }

    fn holdings(&self) -> Option<&[Holding]> {
        self.holdings.as_deref()
    }
}

impl Account for Liability {
    fn account_key(&self) -> &str {
        &self.account_key
    }

    fn institution(&self) -> &str {
        &self.institution
    }

    fn balance(&self) -> Option<f64> {
        self.balance
    }

    fn status(&self) -> &AccountStatus {
        &self.status
    }

    fn net_worth_contribution(&self) -> f64 {
        -self.balance.unwrap_or(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asset(balance: Option<f64>, status: AccountStatus) -> Asset {
        Asset {
            account_key: "sha256:test".into(),
            institution: "Chase".into(),
            r#type: "checking".into(),
            balance,
            status,
            holdings: None,
        }
    }

    fn liability(balance: Option<f64>, status: AccountStatus) -> Liability {
        Liability {
            account_key: "sha256:test".into(),
            institution: "Chase".into(),
            r#type: "credit_card".into(),
            balance,
            status,
        }
    }

    #[test]
    fn asset_contribution_is_positive() {
        let a = asset(Some(4213.55), AccountStatus::Ok);
        assert_eq!(a.net_worth_contribution(), 4213.55);
    }

    #[test]
    fn liability_contribution_is_negative() {
        let l = liability(Some(500.0), AccountStatus::Ok);
        assert_eq!(l.net_worth_contribution(), -500.0);
    }

    #[test]
    fn asset_with_no_balance_contributes_zero() {
        let a = asset(
            None,
            AccountStatus::Error {
                message: "timeout".into(),
            },
        );
        assert_eq!(a.net_worth_contribution(), 0.0);
    }

    #[test]
    fn liability_with_no_balance_contributes_zero() {
        let l = liability(
            None,
            AccountStatus::Error {
                message: "timeout".into(),
            },
        );
        assert_eq!(l.net_worth_contribution(), 0.0);
    }

    #[test]
    fn accessors_expose_underlying_fields() {
        let a = asset(Some(100.0), AccountStatus::Ok);
        assert_eq!(a.account_key(), "sha256:test");
        assert_eq!(a.institution(), "Chase");
        assert_eq!(a.balance(), Some(100.0));
        assert_eq!(a.status(), &AccountStatus::Ok);
    }

    #[test]
    fn an_asset_with_no_holdings_returns_none() {
        let a = asset(Some(100.0), AccountStatus::Ok);
        assert_eq!(a.holdings(), None);
    }

    #[test]
    fn an_asset_with_holdings_returns_them() {
        let mut a = asset(Some(100.0), AccountStatus::Ok);
        a.holdings = Some(vec![Holding {
            symbol: "VOO".into(),
            description: "Vanguard S&P 500 ETF".into(),
            value: 100.0,
        }]);

        let holdings = a.holdings().unwrap();
        assert_eq!(holdings.len(), 1);
        assert_eq!(holdings[0].symbol, "VOO");
    }

    #[test]
    fn a_liability_always_returns_no_holdings() {
        // The trait's default-implemented `holdings()` — Liability
        // never overrides it, since a liability having holdings makes
        // no sense.
        let l = liability(Some(500.0), AccountStatus::Ok);
        assert_eq!(l.holdings(), None);
    }
}
