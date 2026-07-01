use crate::account::{Account, AccountStatus};

/// Net worth for one run (spec §12).
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum NetWorth {
    /// At least one account succeeded this run. This is
    /// sum(asset contributions) + sum(liability contributions) — i.e.
    /// assets minus liabilities, since `Account::net_worth_contribution()`
    /// already carries the sign (D11) — computed over `status: ok`
    /// accounts only.
    Computed(f64),
    /// Every account failed this run (or there were none to begin with).
    /// Never rendered as `$0`, which would be indistinguishable from a
    /// genuine zero net worth (§9.1, §13).
    Unavailable { total_sources: usize },
}

/// Computes net worth over a run's accounts (spec §12). Only
/// `status: ok` accounts contribute; if none succeeded, returns
/// `NetWorth::Unavailable` rather than a numeric `0.0`.
pub fn calculate_net_worth<'a, I>(accounts: I) -> NetWorth
where
    I: IntoIterator<Item = &'a dyn Account>,
{
    let accounts: Vec<&dyn Account> = accounts.into_iter().collect();
    let total_sources = accounts.len();

    let ok_contributions: Vec<f64> = accounts
        .iter()
        .filter(|a| matches!(a.status(), AccountStatus::Ok))
        .map(|a| a.net_worth_contribution())
        .collect();

    if ok_contributions.is_empty() {
        return NetWorth::Unavailable { total_sources };
    }

    NetWorth::Computed(ok_contributions.iter().sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Asset, Liability};

    fn ok_asset(balance: f64) -> Asset {
        Asset {
            account_key: "sha256:test".into(),
            institution: "Chase".into(),
            r#type: "checking".into(),
            balance: Some(balance),
            status: AccountStatus::Ok,
        }
    }

    fn ok_liability(balance: f64) -> Liability {
        Liability {
            account_key: "sha256:test".into(),
            institution: "Chase".into(),
            r#type: "credit_card".into(),
            balance: Some(balance),
            status: AccountStatus::Ok,
        }
    }

    fn errored_asset() -> Asset {
        Asset {
            account_key: "sha256:test".into(),
            institution: "Chase".into(),
            r#type: "checking".into(),
            balance: None,
            status: AccountStatus::Error {
                message: "timeout".into(),
            },
        }
    }

    #[test]
    fn mixed_assets_and_liabilities_nets_correctly() {
        let asset = ok_asset(5000.0);
        let liability = ok_liability(1200.0);
        let accounts: Vec<&dyn Account> = vec![&asset, &liability];
        assert_eq!(calculate_net_worth(accounts), NetWorth::Computed(3800.0));
    }

    #[test]
    fn failed_accounts_are_excluded_from_the_sum() {
        let ok = ok_asset(1000.0);
        let failed = errored_asset();
        let accounts: Vec<&dyn Account> = vec![&ok, &failed];
        assert_eq!(calculate_net_worth(accounts), NetWorth::Computed(1000.0));
    }

    #[test]
    fn all_accounts_failed_returns_unavailable() {
        let failed_one = errored_asset();
        let failed_two = errored_asset();
        let accounts: Vec<&dyn Account> = vec![&failed_one, &failed_two];
        assert_eq!(
            calculate_net_worth(accounts),
            NetWorth::Unavailable { total_sources: 2 }
        );
    }

    #[test]
    fn no_accounts_returns_unavailable() {
        let accounts: Vec<&dyn Account> = vec![];
        assert_eq!(
            calculate_net_worth(accounts),
            NetWorth::Unavailable { total_sources: 0 }
        );
    }

    #[test]
    fn single_ok_asset_nets_to_its_balance() {
        let asset = ok_asset(250.0);
        let accounts: Vec<&dyn Account> = vec![&asset];
        assert_eq!(calculate_net_worth(accounts), NetWorth::Computed(250.0));
    }
}
