//! SimpleFIN plumbing shared by connecting, linking, Refresh and loading
//! earlier transactions.
//!
//! The app keeps one `SimpleFinProvider` for its whole lifetime, so they all
//! share its 60-second account cache and its request budget (the Bridge
//! asks for 24 or fewer requests a day) instead of each sending its own
//! `GET /accounts`. It also keeps the Access URL in memory once read, since
//! every Keychain read can ask for the login password while the app isn't
//! signed (#28).

use std::collections::{BTreeSet, HashMap};
use std::io;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use obol_core::{
    default_account_type, default_category, Account, Category, Credentials, LinkableAccount,
    Provider, ProviderError, ProviderFactory, Secret, SimpleFinClient, SimpleFinProvider,
    SourceConfig,
};
use tokio::sync::Mutex;

use crate::dashboard::StorageLayout;

pub struct SimpleFin {
    pub provider: Arc<SimpleFinProvider>,
    access_url: Mutex<Option<Secret<String>>>,
}

impl SimpleFin {
    /// Requests are counted in `usage_path`, the same log the CLI uses.
    pub fn new(usage_path: PathBuf) -> Self {
        Self {
            provider: Arc::new(SimpleFinProvider::with_usage_log(
                SimpleFinClient::default(),
                usage_path,
            )),
            access_url: Mutex::new(None),
        }
    }

    /// The Access URL: the one remembered this launch, else read once (env
    /// var, then Keychain) and remembered. `None` if there's none, so a
    /// later call tries again.
    pub async fn access_url(&self) -> Option<Secret<String>> {
        // Held across the read, so two commands arriving together cause
        // one Keychain prompt, not two.
        let mut remembered = self.access_url.lock().await;
        if remembered.is_none() {
            *remembered = obol_core::simplefin_access_url();
        }
        remembered.clone()
    }

    /// Keeps a freshly claimed Access URL, so nothing reads it back from
    /// the Keychain this launch.
    pub async fn remember(&self, access_url: Secret<String>) {
        *self.access_url.lock().await = Some(access_url);
    }

    /// The registry a refresh runs with: core's, plus this shared provider.
    pub fn registry(&self) -> HashMap<&'static str, ProviderFactory> {
        let mut registry = obol_core::provider_registry();
        let shared = Arc::clone(&self.provider);
        registry.insert(
            "simplefin",
            Box::new(move || Box::new(Shared(Arc::clone(&shared))) as Box<dyn Provider>),
        );
        registry
    }
}

/// The engine wants a fresh `Box<dyn Provider>` per run; this hands it the
/// one shared provider instead.
struct Shared(Arc<SimpleFinProvider>);

#[async_trait]
impl Provider for Shared {
    async fn fetch(
        &self,
        source: &SourceConfig,
        credentials: Option<&Credentials>,
    ) -> Result<Vec<Box<dyn Account>>, ProviderError> {
        self.0.fetch(source, credentials).await
    }
}

/// Obol's guess for an unlinked account: asset or liability from the
/// balance's sign (the name when it's zero), then a type that fits it. The
/// connect form starts from this, and Refresh links new accounts with it.
pub fn guess(account: &LinkableAccount) -> (Category, &'static str) {
    let category = default_category(&account.balance, &account.name);
    (category, default_account_type(&account.name, category))
}

/// SimpleFIN account ids the user chose not to link. Discovery never adds
/// them, and the connect form shows them unticked. Missing means none.
pub fn read_ignored(layout: &StorageLayout) -> Result<BTreeSet<String>, String> {
    match std::fs::read(&layout.simplefin_ignored) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|err| format!("simplefin_ignored.json could not be parsed: {err}")),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(BTreeSet::new()),
        Err(err) => Err(format!("simplefin_ignored.json could not be read: {err}")),
    }
}

pub fn write_ignored(layout: &StorageLayout, ignored: &BTreeSet<String>) -> Result<(), String> {
    let json = serde_json::to_vec_pretty(ignored).map_err(|err| err.to_string())?;
    obol_core::write_atomically(&layout.simplefin_ignored, &json)
        .map_err(|err| format!("simplefin_ignored.json could not be saved: {err}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::path::Path;

    fn temp_layout(name: &str) -> StorageLayout {
        let root = std::env::temp_dir().join(format!(
            "obol-desktop-simplefin-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        StorageLayout::under(&root)
    }

    fn linkable(name: &str, balance: &str) -> LinkableAccount {
        LinkableAccount {
            account_id: "ACT-1".into(),
            name: name.into(),
            institution: "Chase Bank".into(),
            balance: balance.into(),
        }
    }

    #[test]
    fn should_register_the_simplefin_provider() {
        let simplefin = SimpleFin::new(Path::new("/root/simplefin_usage.json").into());

        assert!(simplefin.registry().contains_key("simplefin"));
    }

    #[test]
    fn should_hand_every_run_the_same_shared_provider() {
        let simplefin = SimpleFin::new(Path::new("/root/simplefin_usage.json").into());
        let registry = simplefin.registry();

        let _first = registry["simplefin"]();
        let _second = registry["simplefin"]();

        // Ours, the registry closure's, and one per provider handed out.
        assert_eq!(Arc::strong_count(&simplefin.provider), 4);
    }

    #[tokio::test]
    async fn should_hand_back_a_remembered_access_url() {
        use obol_core::ExposeSecret;
        let simplefin = SimpleFin::new(Path::new("/root/simplefin_usage.json").into());
        simplefin
            .remember(Secret::new("https://remembered".to_string()))
            .await;

        let url = simplefin.access_url().await;

        assert_eq!(
            url.map(|url| url.expose_secret().clone()),
            Some("https://remembered".to_string())
        );
    }

    #[test]
    fn should_guess_a_liability_from_a_negative_balance() {
        assert_eq!(
            guess(&linkable("Sapphire Card", "-120.50")),
            (Category::Liability, "credit_card")
        );
    }

    #[test]
    fn should_guess_an_asset_type_from_the_name() {
        assert_eq!(
            guess(&linkable("Total Checking", "2400.00")),
            (Category::Asset, "checking")
        );
    }

    #[test]
    fn should_read_no_ignored_accounts_when_the_file_is_missing() {
        let layout = temp_layout("ignored-missing");

        assert_eq!(read_ignored(&layout), Ok(BTreeSet::new()));
    }

    #[test]
    fn should_round_trip_ignored_accounts() {
        let layout = temp_layout("ignored-round-trip");
        let ignored: BTreeSet<String> = ["ACT-1".to_string(), "ACT-2".to_string()].into();

        write_ignored(&layout, &ignored).unwrap();

        assert_eq!(read_ignored(&layout), Ok(ignored));
    }

    #[test]
    fn should_report_a_malformed_ignored_file() {
        let layout = temp_layout("ignored-malformed");
        fs::write(&layout.simplefin_ignored, "not json").unwrap();

        assert!(read_ignored(&layout).is_err());
    }
}
