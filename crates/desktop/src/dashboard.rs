//! The two things the balances screen does (spec §16 v0.3a, D43): show
//! what's already on disk, and fetch fresh. Kept apart from the
//! `#[tauri::command]` wrappers in `lib.rs` so both are testable against
//! a temp directory with no window involved.

use std::collections::HashMap;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use obol_core::{CredentialSource, Credentials, ProviderFactory, SourceConfig};
use serde::Serialize;

use crate::view::{self, DashboardView};

/// Where obol's files live under one storage root. File names must match
/// the CLI's exactly (`crates/cli/src/main.rs`) — both frontends read and
/// write the same directory, so a divergence would leave each looking at
/// data the other never wrote.
#[derive(Debug, Clone, PartialEq)]
pub struct StorageLayout {
    pub sources: PathBuf,
    pub snapshots: PathBuf,
    pub processed_statements: PathBuf,
    pub rules: PathBuf,
}

impl StorageLayout {
    pub fn under(root: &Path) -> Self {
        Self {
            sources: root.join("sources.yaml"),
            snapshots: root.join("snapshots"),
            processed_statements: root.join("processed_statements.json"),
            rules: root.join("rules.yaml"),
        }
    }
}

/// `OBOL_STORAGE_DIR` if set, else `~/Library/Application Support/obol`
/// (§4, §11.1). The override is a dev seam for pointing the app at
/// fixtures (D43), not user configuration — the storage path stays fixed
/// in v1 (D17). Takes the two values as arguments rather than reading the
/// environment itself, so it's testable without mutating process-global
/// state that other tests share.
pub fn resolve_storage_root(
    override_dir: Option<OsString>,
    home: Option<OsString>,
) -> Result<PathBuf, String> {
    // An empty value counts as unset: `PathBuf::from("")` is a relative
    // path, which would quietly read and write in the working directory.
    if let Some(dir) = override_dir.filter(|dir| !dir.is_empty()) {
        return Ok(PathBuf::from(dir));
    }
    let home = home
        .filter(|home| !home.is_empty())
        .ok_or_else(|| "HOME is not set, so obol's storage directory can't be found".to_string())?;
    Ok(PathBuf::from(home).join("Library/Application Support/obol"))
}

/// The newest snapshot on disk, diffed against the one before it. `None`
/// on a fresh install with no history yet.
pub fn load_dashboard(layout: &StorageLayout) -> Result<Option<DashboardView>, String> {
    let recent = obol_core::load_recent_snapshots(&layout.snapshots, 2)
        .map_err(|err| format!("snapshot history could not be read: {err}"))?;
    Ok(recent
        .first()
        .map(|newest| view::build(newest, recent.get(1))))
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct RefreshResult {
    pub view: DashboardView,
    /// §9.1: persistence is best-effort, so a failed save never blocks
    /// showing what was just fetched — it's surfaced as a warning instead.
    pub save_warning: Option<String>,
}

/// Fetches every configured source, saves the result, and renders it.
pub async fn refresh_dashboard(
    layout: &StorageLayout,
    registry: &HashMap<&'static str, ProviderFactory>,
) -> Result<RefreshResult, String> {
    // §9.1: a malformed sources.yaml blocks the run outright rather than
    // falling back to an empty source list.
    let sources = obol_core::load_or_init(&layout.sources)
        .map_err(|err| format!("sources.yaml could not be loaded: {err}"))?;

    // §6.2: never snapshot an empty source list — the CLI routes this case
    // to the Sources screen instead. Fetching anyway would save an empty
    // snapshot that becomes the newest in history, so every later launch
    // would open on a blank dashboard with the real history hidden behind
    // it. Returning an error leaves the last good view on screen.
    if sources.is_empty() {
        return Err("no sources are configured, so there's nothing to refresh. \
             Add one with `obol sources` in a terminal."
            .to_string());
    }

    // Captured before the run saves, matching the TUI: read afterwards,
    // it would be the snapshot this refresh just wrote. Unreadable history
    // only costs the delta, so it degrades to "no previous" rather than
    // failing a refresh that can otherwise succeed.
    let previous = obol_core::load_recent_snapshots(&layout.snapshots, 1)
        .unwrap_or_default()
        .into_iter()
        .next();

    let result = obol_core::run_and_save(
        &sources,
        registry,
        &NoInteractiveProviders,
        &layout.snapshots,
    )
    .await;

    Ok(RefreshResult {
        view: view::build(&result.snapshot, previous.as_ref()),
        save_warning: result
            .save_error
            // Shown on its own as a banner, so phrased as a full sentence.
            .map(|err| format!("This refresh was not saved to history: {err}")),
    })
}

/// The providers a real refresh can reach on this branch.
///
/// Duplicates the CLI's private `register_statement_import`
/// (`crates/cli/src/main.rs`) rather than moving it into core, because
/// both of the files that move touches are being changed by the SimpleFIN
/// migration (#37) in parallel. Once #37 merges, this belongs in core
/// beside its `register_simplefin`, called from both frontends.
pub fn production_registry(layout: &StorageLayout) -> HashMap<&'static str, ProviderFactory> {
    let mut registry = obol_core::provider_registry();
    let ledger_path = layout.processed_statements.clone();
    let rules_path = layout.rules.clone();
    registry.insert(
        "statement_import",
        Box::new(move || {
            Box::new(obol_core::StatementImportProvider::new(
                ledger_path.clone(),
                rules_path.clone(),
            )) as Box<dyn obol_core::Provider>
        }),
    );
    registry
}

/// No interactive provider (webdriver, manual entry) exists yet, so
/// there's nothing to prompt for. Same placeholder as the CLI's, and
/// duplicated for the same reason as [`production_registry`].
struct NoInteractiveProviders;

impl CredentialSource for NoInteractiveProviders {
    fn provide(&self, _source: &SourceConfig) -> Option<Credentials> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{asset, snapshot_at};
    use crate::view::NetWorthView;
    use obol_core::{Account, AccountStatus, Asset, Provider, ProviderError};
    use serde_json::json;
    use std::fs;
    use std::time::{Duration, SystemTime};

    /// Per-test directory under the system temp dir, cleared first so a
    /// re-run never sees a previous run's files. Same idiom as core's own
    /// storage tests.
    fn temp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("obol-desktop-test-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// Saves a one-account snapshot and backdates its mtime.
    /// `load_recent_snapshots` orders by mtime, so ordering is set
    /// explicitly rather than relying on write order or sleeping between
    /// writes. Each call needs a distinct date: snapshots are one file per
    /// UTC day, and a same-day save overwrites.
    fn save_snapshot_aged(dir: &Path, created_at: &str, balance: f64, age: Duration) {
        let path = obol_core::save_snapshot(
            dir,
            &snapshot_at(created_at, json!([asset("chase", balance)])),
        )
        .unwrap();
        fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_modified(SystemTime::now() - age)
            .unwrap();
    }

    /// A provider that always reports one asset at a fixed balance — lets
    /// refresh be tested end to end without any real institution.
    struct FixedBalance(f64);

    #[async_trait::async_trait]
    impl Provider for FixedBalance {
        async fn fetch(
            &self,
            source: &SourceConfig,
            _credentials: Option<&Credentials>,
        ) -> Result<Vec<Box<dyn Account>>, ProviderError> {
            Ok(vec![Box::new(Asset {
                account_key: source.id.clone(),
                institution: source.institution.clone(),
                r#type: source.account_type.clone(),
                balance: Some(self.0),
                status: AccountStatus::Ok,
                holdings: None,
            })])
        }
    }

    fn fixed_balance_registry(balance: f64) -> HashMap<&'static str, ProviderFactory> {
        let mut registry = obol_core::provider_registry();
        registry.insert(
            "fixed",
            Box::new(move || Box::new(FixedBalance(balance)) as Box<dyn Provider>),
        );
        registry
    }

    fn write_fixed_source(layout: &StorageLayout) {
        fs::write(
            &layout.sources,
            "sources:\n  - id: chase_checking\n    provider: fixed\n    category: asset\n    type: checking\n    institution: Chase\n    account_salt: test-salt\n",
        )
        .unwrap();
    }

    // --- resolve_storage_root ---

    #[test]
    fn should_prefer_storage_override_when_set() {
        let root = resolve_storage_root(Some("/tmp/fixtures".into()), Some("/Users/k".into()));

        assert_eq!(root, Ok(PathBuf::from("/tmp/fixtures")));
    }

    #[test]
    fn should_fall_back_to_application_support_under_home() {
        let root = resolve_storage_root(None, Some("/Users/k".into()));

        assert_eq!(
            root,
            Ok(PathBuf::from("/Users/k/Library/Application Support/obol"))
        );
    }

    #[test]
    fn should_ignore_empty_storage_override() {
        // `OBOL_STORAGE_DIR=` set but empty would otherwise resolve to the
        // process's working directory and read or write snapshots there.
        let root = resolve_storage_root(Some("".into()), Some("/Users/k".into()));

        assert_eq!(
            root,
            Ok(PathBuf::from("/Users/k/Library/Application Support/obol"))
        );
    }

    #[test]
    fn should_error_when_neither_override_nor_home_is_set() {
        let root = resolve_storage_root(None, None);

        assert!(root.is_err());
    }

    #[test]
    fn should_error_when_home_is_empty() {
        let root = resolve_storage_root(None, Some("".into()));

        assert!(root.is_err());
    }

    // --- StorageLayout ---

    #[test]
    fn should_lay_out_files_where_the_cli_expects_them() {
        let layout = StorageLayout::under(Path::new("/root"));

        assert_eq!(
            layout,
            StorageLayout {
                sources: "/root/sources.yaml".into(),
                snapshots: "/root/snapshots".into(),
                processed_statements: "/root/processed_statements.json".into(),
                rules: "/root/rules.yaml".into(),
            }
        );
    }

    // --- load_dashboard ---

    #[test]
    fn should_return_none_when_no_snapshots_exist() {
        let layout = StorageLayout::under(&temp_root("load-empty"));

        assert_eq!(load_dashboard(&layout), Ok(None));
    }

    #[test]
    fn should_error_when_snapshot_history_is_unreadable() {
        // Must not collapse into `Ok(None)`: that means "fresh install,
        // press Refresh", which would hide a broken directory behind a
        // misleading empty state.
        let layout = StorageLayout::under(&temp_root("load-unreadable"));
        fs::write(&layout.snapshots, "not a directory").unwrap();

        assert!(load_dashboard(&layout).is_err());
    }

    #[test]
    fn should_build_view_from_newest_snapshot() {
        let layout = StorageLayout::under(&temp_root("load-newest"));
        save_snapshot_aged(
            &layout.snapshots,
            "2026-10-19T09:00:00Z",
            1000.0,
            Duration::from_secs(7200),
        );
        save_snapshot_aged(
            &layout.snapshots,
            "2026-10-26T09:00:00Z",
            1100.0,
            Duration::from_secs(60),
        );

        let view = load_dashboard(&layout).unwrap().unwrap();

        assert_eq!(view.created_at, "2026-10-26T09:00:00Z");
    }

    #[test]
    fn should_diff_newest_against_the_one_before_it() {
        let layout = StorageLayout::under(&temp_root("load-delta"));
        save_snapshot_aged(
            &layout.snapshots,
            "2026-10-19T09:00:00Z",
            1000.0,
            Duration::from_secs(7200),
        );
        save_snapshot_aged(
            &layout.snapshots,
            "2026-10-26T09:00:00Z",
            1100.0,
            Duration::from_secs(60),
        );

        let view = load_dashboard(&layout).unwrap().unwrap();

        assert_eq!(
            view.delta.map(|d| (d.amount, d.since)),
            Some((100.0, "2026-10-19T09:00:00Z".to_string()))
        );
    }

    // --- refresh_dashboard ---

    #[tokio::test]
    async fn should_render_fetched_balances_on_refresh() {
        let layout = StorageLayout::under(&temp_root("refresh-render"));
        write_fixed_source(&layout);

        let result = refresh_dashboard(&layout, &fixed_balance_registry(500.0))
            .await
            .unwrap();

        assert_eq!(
            result.view.net_worth,
            NetWorthView::Computed { value: 500.0 }
        );
    }

    #[tokio::test]
    async fn should_save_a_snapshot_on_refresh() {
        let layout = StorageLayout::under(&temp_root("refresh-save"));
        write_fixed_source(&layout);

        refresh_dashboard(&layout, &fixed_balance_registry(500.0))
            .await
            .unwrap();

        assert_eq!(
            obol_core::load_recent_snapshots(&layout.snapshots, 10)
                .unwrap()
                .len(),
            1
        );
    }

    #[tokio::test]
    async fn should_diff_refresh_against_snapshot_from_before_the_run() {
        // "Previous" must be captured *before* the run saves. Read after,
        // it would be the snapshot this refresh just wrote, and the delta
        // would always be zero.
        let layout = StorageLayout::under(&temp_root("refresh-delta"));
        write_fixed_source(&layout);
        save_snapshot_aged(
            &layout.snapshots,
            "2026-01-01T09:00:00Z",
            400.0,
            Duration::from_secs(7200),
        );

        let result = refresh_dashboard(&layout, &fixed_balance_registry(500.0))
            .await
            .unwrap();

        assert_eq!(
            result.view.delta.map(|d| (d.amount, d.since)),
            Some((100.0, "2026-01-01T09:00:00Z".to_string()))
        );
    }

    #[tokio::test]
    async fn should_surface_save_failure_without_discarding_the_fetch() {
        // A plain file where the snapshots directory should be makes the
        // save fail. §9.1: the fetch is still shown, with a warning.
        let layout = StorageLayout::under(&temp_root("refresh-save-fails"));
        write_fixed_source(&layout);
        fs::write(&layout.snapshots, "not a directory").unwrap();

        let result = refresh_dashboard(&layout, &fixed_balance_registry(500.0))
            .await
            .unwrap();

        assert!(result.save_warning.is_some());
    }

    #[tokio::test]
    async fn should_report_no_save_warning_when_save_succeeds() {
        let layout = StorageLayout::under(&temp_root("refresh-save-ok"));
        write_fixed_source(&layout);

        let result = refresh_dashboard(&layout, &fixed_balance_registry(500.0))
            .await
            .unwrap();

        assert_eq!(result.save_warning, None);
    }

    #[tokio::test]
    async fn should_refuse_to_refresh_with_no_sources() {
        // §6.2: never run a snapshot against an empty source list. The
        // CLI routes that case away from fetching; refresh must too.
        let layout = StorageLayout::under(&temp_root("refresh-no-sources"));

        let result = refresh_dashboard(&layout, &fixed_balance_registry(500.0)).await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn should_not_save_a_snapshot_when_there_are_no_sources() {
        // An empty snapshot would become the newest in history, so the
        // next launch would open on a blank dashboard and hide every real
        // snapshot behind it.
        let layout = StorageLayout::under(&temp_root("refresh-no-sources-save"));

        let _ = refresh_dashboard(&layout, &fixed_balance_registry(500.0)).await;

        assert_eq!(
            obol_core::load_recent_snapshots(&layout.snapshots, 10)
                .unwrap()
                .len(),
            0
        );
    }

    #[tokio::test]
    async fn should_error_when_sources_yaml_is_malformed() {
        // §9.1: a broken sources.yaml blocks the run with a clear message
        // rather than falling back to an empty source list.
        let layout = StorageLayout::under(&temp_root("refresh-bad-yaml"));
        fs::write(&layout.sources, "sources: [ this is: not valid").unwrap();

        let result = refresh_dashboard(&layout, &fixed_balance_registry(500.0)).await;

        assert!(result.is_err());
    }

    // --- production_registry ---

    #[test]
    fn should_register_statement_import_provider() {
        let registry = production_registry(&StorageLayout::under(Path::new("/root")));

        assert!(registry.contains_key("statement_import"));
    }
}
