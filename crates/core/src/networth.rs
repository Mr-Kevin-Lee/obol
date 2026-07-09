use crate::account::{Account, AccountStatus, Asset, Liability};
use crate::snapshot::{AccountRecord, Category, Status};

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

/// Computes net worth over a **loaded/fetched `Snapshot`'s**
/// `AccountRecord`s (spec §12) — the counterpart to [`calculate_net_worth`]
/// for the storage-DTO shape rather than live `Box<dyn Account>` fetch
/// results. These two shapes exist for good reason (§11's domain/storage
/// split), but net worth's sign rule — positive for assets, negative for
/// liabilities — must still live in exactly one place (D11). Rather than
/// re-deriving that rule here a second time, each record is adapted into
/// a throwaway `Asset`/`Liability` value and run through
/// `Account::net_worth_contribution()`, the same as a live fetch would.
pub fn calculate_net_worth_from_records(records: &[AccountRecord]) -> NetWorth {
    let accounts: Vec<Box<dyn Account>> = records.iter().map(record_to_account).collect();
    calculate_net_worth(accounts.iter().map(|a| a.as_ref()))
}

fn record_to_account(record: &AccountRecord) -> Box<dyn Account> {
    let status = match record.status() {
        Status::Ok => AccountStatus::Ok,
        Status::Error | Status::Unknown => AccountStatus::Error {
            message: record
                .error_message()
                .unwrap_or("unknown error")
                .to_string(),
        },
    };
    let account_key = record.account_key().to_string();
    let institution = record.institution().to_string();
    let r#type = record.account_type().to_string();
    let balance = record.balance();

    match record.category() {
        Category::Liability => Box::new(Liability {
            account_key,
            institution,
            r#type,
            balance,
            status,
        }),
        // An `Unknown` category (D14's forward-compat fallback) has no
        // sign-correct treatment available — arbitrarily but
        // consistently defaulting to the Asset side rather than
        // silently dropping it from the total. A newer schema
        // introducing a category this build doesn't understand yet is
        // an edge case a v1 net worth figure can't get perfectly right
        // either way.
        Category::Asset | Category::Unknown => Box::new(Asset {
            account_key,
            institution,
            r#type,
            balance,
            status,
            holdings: record.holdings().map(|h| h.to_vec()),
        }),
    }
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
            holdings: None,
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
            holdings: None,
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

    fn ok_record(category: Category, balance: f64) -> AccountRecord {
        AccountRecord {
            account_key: "sha256:test".into(),
            source_id: "chase_checking".into(),
            institution: "Chase".into(),
            category,
            account_type: "checking".into(),
            balance: Some(balance),
            currency: "USD".into(),
            status: Status::Ok,
            error_message: None,
            holdings: None,
        }
    }

    fn errored_record(category: Category) -> AccountRecord {
        AccountRecord {
            account_key: "sha256:test".into(),
            source_id: "chase_checking".into(),
            institution: "Chase".into(),
            category,
            account_type: "checking".into(),
            balance: None,
            currency: "USD".into(),
            status: Status::Error,
            error_message: Some("timeout".into()),
            holdings: None,
        }
    }

    #[test]
    fn from_records_mixed_assets_and_liabilities_nets_correctly() {
        let records = vec![
            ok_record(Category::Asset, 5000.0),
            ok_record(Category::Liability, 1200.0),
        ];
        assert_eq!(
            calculate_net_worth_from_records(&records),
            NetWorth::Computed(3800.0)
        );
    }

    #[test]
    fn from_records_excludes_failed_accounts_from_the_sum() {
        let records = vec![
            ok_record(Category::Asset, 1000.0),
            errored_record(Category::Asset),
        ];
        assert_eq!(
            calculate_net_worth_from_records(&records),
            NetWorth::Computed(1000.0)
        );
    }

    #[test]
    fn from_records_all_failed_returns_unavailable() {
        let records = vec![
            errored_record(Category::Asset),
            errored_record(Category::Liability),
        ];
        assert_eq!(
            calculate_net_worth_from_records(&records),
            NetWorth::Unavailable { total_sources: 2 }
        );
    }

    #[test]
    fn from_records_empty_returns_unavailable() {
        assert_eq!(
            calculate_net_worth_from_records(&[]),
            NetWorth::Unavailable { total_sources: 0 }
        );
    }

    #[test]
    fn from_records_unknown_category_is_treated_as_a_positive_contribution() {
        let records = vec![ok_record(Category::Unknown, 100.0)];
        assert_eq!(
            calculate_net_worth_from_records(&records),
            NetWorth::Computed(100.0)
        );
    }

    #[test]
    fn from_records_matches_calculate_net_worth_for_the_same_data() {
        let asset = ok_asset(5000.0);
        let liability = ok_liability(1200.0);
        let accounts: Vec<&dyn Account> = vec![&asset, &liability];
        let via_accounts = calculate_net_worth(accounts);

        let records = vec![
            ok_record(Category::Asset, 5000.0),
            ok_record(Category::Liability, 1200.0),
        ];
        let via_records = calculate_net_worth_from_records(&records);

        assert_eq!(via_accounts, via_records);
    }
}
