//! Guards the committed fixture sets that `make desktop-dev` launches
//! against (D43's mock seam). Hand-written JSON only fails at runtime, so
//! each set is loaded through the same path the app uses: a typo, or a
//! core schema change the fixtures haven't caught up with, fails here
//! instead of as a blank window.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use obol_desktop::dashboard::{load_dashboard, StorageLayout};
use obol_desktop::view::{DashboardView, NetWorthView};

const DAY: Duration = Duration::from_secs(86_400);

/// Tests run as parallel threads of one process, and several stage the
/// same fixture set, so the process id alone can't keep their scratch
/// directories apart: one test's cleanup would interleave with another's
/// copy and scramble the mtime order this harness exists to set.
static NEXT_SCRATCH: AtomicUsize = AtomicUsize::new(0);

/// Copies a fixture set to scratch, giving its snapshots mtimes in
/// filename order. Git doesn't preserve mtimes and snapshot history is
/// ordered by mtime, so a fresh checkout would otherwise order them
/// arbitrarily. `scripts/desktop_dev.sh` does the same before launching.
fn stage(fixture: &str) -> StorageLayout {
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(fixture);
    assert!(source.is_dir(), "no fixture set named {fixture}");

    let root = std::env::temp_dir().join(format!(
        "obol-desktop-fixture-{fixture}-{}-{}",
        std::process::id(),
        NEXT_SCRATCH.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let layout = StorageLayout::under(&root);

    let snapshots = source.join("snapshots");
    if snapshots.is_dir() {
        fs::create_dir_all(&layout.snapshots).unwrap();
        // Filenames are YYYY-MM-DD, so lexical order is chronological.
        let mut names: Vec<_> = fs::read_dir(&snapshots)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        let oldest = SystemTime::now() - DAY * names.len() as u32;
        for (age_rank, name) in names.iter().enumerate() {
            let dest = layout.snapshots.join(name);
            fs::copy(snapshots.join(name), &dest).unwrap();
            fs::File::options()
                .write(true)
                .open(&dest)
                .unwrap()
                .set_modified(oldest + DAY * age_rank as u32)
                .unwrap();
        }
    }

    layout
}

fn dashboard(fixture: &str) -> DashboardView {
    load_dashboard(&stage(fixture))
        .unwrap()
        .expect("fixture set should contain at least one snapshot")
}

#[test]
fn typical_fixture_should_compute_net_worth_from_ok_accounts_only() {
    // 8,420.25 + 52,310.00 − 1,245.50 − 3,102.75. The failed Fidelity
    // account is excluded (§12). Quarter-dollar balances keep the sum
    // exact in binary floating point, so this can assert equality.
    assert_eq!(
        dashboard("typical").net_worth,
        NetWorthView::Computed { value: 56_382.0 }
    );
}

#[test]
fn typical_fixture_should_diff_against_the_previous_week() {
    let delta = dashboard("typical")
        .delta
        .expect("typical fixture should have a predecessor");

    assert_eq!(
        (delta.amount, delta.since.as_str()),
        (1_112.75, "2026-10-19T09:00:00-07:00")
    );
}

#[test]
fn typical_fixture_should_include_a_failed_account() {
    // FR18's "data unavailable" panel needs something to render.
    assert!(dashboard("typical")
        .assets
        .iter()
        .any(|account| account.error_message.is_some()));
}

#[test]
fn typical_fixture_should_include_every_holdings_class() {
    // FR22's bars need all three asset classes to be worth looking at.
    let labels: Vec<String> = dashboard("typical")
        .assets
        .iter()
        .filter_map(|account| account.holdings.as_ref())
        .flatten()
        .map(|bucket| bucket.label.clone())
        .collect();

    assert_eq!(labels, ["Cash", "Fund", "Individual Stock"]);
}

#[test]
fn all_failed_fixture_should_report_net_worth_unavailable() {
    // §9.1: never a numeric $0 when every source failed.
    assert_eq!(
        dashboard("all-failed").net_worth,
        NetWorthView::Unavailable { total_sources: 2 }
    );
}

#[test]
fn empty_fixture_should_have_no_dashboard() {
    // A fresh install: no history yet, so the window shows its empty state.
    assert_eq!(load_dashboard(&stage("empty")), Ok(None));
}
