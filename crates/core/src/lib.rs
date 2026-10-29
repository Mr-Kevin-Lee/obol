mod account;
mod checklist;
mod checklist_storage;
mod debt_payoff;
mod debt_payoff_storage;
mod emergency_fund;
mod emergency_fund_storage;
mod engine;
mod keychain;
mod lock;
mod migration;
mod monthly_spend;
mod monthly_spend_storage;
mod networth;
mod pii;
mod provider;
mod retry;
mod rules_storage;
mod simplefin;
mod simplefin_link;
mod simplefin_provider;
mod simplefin_usage;
mod snapshot;
mod sources;
mod storage;
mod threshold_band;

pub use account::{Account, AccountStatus, Asset, Holding, Liability};
pub use checklist::{
    completion_summary, status_for, ChecklistItem, ChecklistItemStatus, ChecklistStatuses,
    CHECKLIST_ITEMS,
};
pub use checklist_storage::{load_or_init_checklist_statuses, set_checklist_item_status};
pub use debt_payoff::{
    evaluate_debt_payoff_priority, DebtInterestRates, DebtPayoffConfig, DebtPayoffStatus,
    FlaggedDebt,
};
pub use debt_payoff_storage::{
    load_or_init_debt_payoff_config, save_debt_payoff_config, save_debt_payoff_interest_rate,
};
pub use emergency_fund::{
    band_for, calculate_emergency_fund_status, EmergencyFundStatus, EmergencyFundThresholds,
};
pub use emergency_fund_storage::{
    load_or_init_emergency_fund_thresholds, save_emergency_fund_thresholds,
};
pub use engine::{run, run_and_save, CredentialSource, RunAndSaveResult};
pub use keychain::{
    probe_keychain_writable, read_simplefin_access_url, store_simplefin_access_url, KeychainError,
};
pub use lock::{acquire_with_timeout, FileLock, LockError};
pub use migration::{load_snapshot_json, LoadedSnapshot, MigrationError, CURRENT_SCHEMA_VERSION};
pub use monthly_spend::{
    band_for_spend, calculate_current_period_spend, extract_spend_series, CurrentPeriodSpend,
    MonthlySpendThresholds, SpendPoint, HISTORY_LIMIT,
};
pub use monthly_spend_storage::{
    load_or_init_monthly_spend_thresholds, save_monthly_spend_thresholds,
};
pub use networth::{calculate_net_worth, calculate_net_worth_from_records, NetWorth};
pub use pii::{scrub, RawAccountData};
pub use provider::{
    provider_registry, Credentials, Provider, ProviderError, ProviderFactory, SourceConfig,
};
pub use retry::{with_retry, RetryConfig, RetryableError};
pub use rules_storage::RulesStorageError;
pub use simplefin::{
    AccountSet, SimpleFinAccount, SimpleFinApiError, SimpleFinClient, SimpleFinConnection,
    SimpleFinError,
};
pub use simplefin_link::{
    default_account_type, default_category, guess_account, linkable_accounts, new_source,
    normalize_account_type, LinkableAccount,
};
pub use simplefin_provider::{
    register as register_simplefin, sanitize as sanitize_server_message, SimpleFinProvider,
};
pub use snapshot::{AccountRecord, Category, Snapshot, Status};
pub use sources::{add_source, edit_source, load_or_init, remove_source, SourcesError};
pub use storage::{load_recent_snapshots, load_snapshot, save_snapshot, StorageError};
pub use threshold_band::ThresholdBand;
