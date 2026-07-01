/// Common interface over asset and liability holdings (decision D11, spec §11).
///
/// `Asset` and `Liability` implement this rather than being distinguished by
/// a `category` field branched on throughout net worth calc and dashboard
/// rendering.
pub trait Account {
    fn account_key(&self) -> &str;
    fn institution(&self) -> &str;
    fn balance(&self) -> Option<f64>;
    fn status(&self) -> &AccountStatus;

    /// Signed contribution to net worth — positive for assets, negative
    /// for liabilities. The one place this sign logic lives.
    fn net_worth_contribution(&self) -> f64;
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
}
