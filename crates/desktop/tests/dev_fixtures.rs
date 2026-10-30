//! Guards the committed fixture sets that `make desktop-dev` launches
//! against (D43's mock seam). Hand-written JSON only fails at runtime, so
//! each set is loaded through the same path the app uses: a typo, or a
//! core schema change the fixtures haven't caught up with, fails here
//! instead of as a blank window.

use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use obol_core::{parse_iso_date, PaymentStatus, YearMonth, TRANSFER};
use obol_desktop::budget::{
    spending_plan_view, transactions_view, SpendingPlanView, TransactionsView,
};
use obol_desktop::dashboard::{load_dashboard, StorageLayout};
use obol_desktop::view::{DashboardView, NetWorthView};

const DAY: Duration = Duration::from_secs(86_400);

/// Tests run as parallel threads of one process, and several stage the
/// same fixture set, so the process id alone can't keep their scratch
/// directories apart: one test's cleanup would interleave with another's
/// copy and scramble the mtime order this harness exists to set.
static NEXT_SCRATCH: AtomicUsize = AtomicUsize::new(0);

/// Copies a fixture set's whole tree to scratch — snapshots, transactions,
/// rules and plan — then gives its snapshots mtimes in filename order. Git
/// doesn't preserve mtimes and snapshot history is ordered by mtime, so a
/// fresh checkout would otherwise order them arbitrarily.
/// `scripts/desktop_dev.sh` does the same before launching.
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
    copy_tree(&source, &root);
    let layout = StorageLayout::under(&root);

    if layout.snapshots.is_dir() {
        // Filenames are YYYY-MM-DD, so lexical order is chronological.
        let mut names: Vec<_> = fs::read_dir(&layout.snapshots)
            .unwrap()
            .map(|entry| entry.unwrap().file_name())
            .collect();
        names.sort();
        let oldest = SystemTime::now() - DAY * names.len() as u32;
        for (age_rank, name) in names.iter().enumerate() {
            fs::File::options()
                .write(true)
                .open(layout.snapshots.join(name))
                .unwrap()
                .set_modified(oldest + DAY * age_rank as u32)
                .unwrap();
        }
    }

    layout
}

fn copy_tree(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// The typical fixture's October 2026, viewed as of the 26th — the date its
/// hand-computed figures assume (5 days left in the month).
fn october_plan() -> SpendingPlanView {
    spending_plan_view(
        &stage("typical"),
        YearMonth::new(2026, 10).unwrap(),
        parse_iso_date("2026-10-26").unwrap(),
    )
    .unwrap()
}

fn october_transactions() -> TransactionsView {
    transactions_view(&stage("typical"), YearMonth::new(2026, 10).unwrap()).unwrap()
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
fn typical_fixture_should_subtotal_each_group_from_ok_accounts() {
    // Assets exclude the failed Fidelity source; the two subtotals
    // reconcile with the net worth above: 60,730.25 − 4,348.25 = 56,382.
    let view = dashboard("typical");

    assert_eq!(
        (view.assets_subtotal, view.liabilities_subtotal),
        (60_730.25, 4_348.25)
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

// --- The typical fixture's October 2026 spending plan (§13.6) ---
//
// Hand-computed from the fixture files, as of 2026-10-26. Every amount is a
// multiple of $0.25, so the sums are exact in binary floating point and
// these assert equality, not closeness.

#[test]
fn typical_plan_should_count_payroll_and_the_tax_refund_as_income() {
    // ACME PAYROLL series 4,500 (paid) + TAX REFUND, an Income-category
    // inflow outside any series, 250.
    assert_eq!(october_plan().income, 4_750.0);
}

#[test]
fn typical_plan_should_count_paid_and_unpaid_bills() {
    // Paid at actual: rent 2,400 + Comcast 82.50 + Netflix 15.50. Unpaid at
    // expected: City Water 45 (past due) + State Farm 125 (upcoming).
    assert_eq!(october_plan().bills, 2_668.0);
}

#[test]
fn typical_plan_should_count_each_target_at_the_larger_of_target_and_spent() {
    // Groceries max(600, 452.75) + Dining max(300, 67.50)
    // + Auto & Transport max(200, 225.25).
    assert_eq!(october_plan().planned, 1_125.25);
}

#[test]
fn typical_plan_should_count_untargeted_spending_as_other() {
    // Shopping 80 − 20 refund + Entertainment 32.50 (Netflix is a bill)
    // + Health 24.25 + Uncategorized 18.
    assert_eq!(october_plan().other, 134.75);
}

#[test]
fn typical_plan_should_leave_income_minus_everything_committed() {
    // 4,750 − 2,668 − 1,125.25 − 134.75.
    assert_eq!(october_plan().left, 822.0);
}

#[test]
fn typical_plan_should_commit_bills_planned_and_other_against_income() {
    // All three terms are non-zero here, so dropping any one of them from
    // the bar's percentage fails this — the unit test only has Other.
    assert_eq!(
        october_plan().committed_percentage,
        Some((2_668.0 + 1_125.25 + 134.75) / 4_750.0 * 100.0)
    );
}

#[test]
fn typical_plan_should_spread_what_is_left_over_the_remaining_days() {
    // 822 over Oct 27–31.
    assert_eq!(october_plan().per_day, Some(164.4));
}

#[test]
fn typical_plan_should_report_each_series_status() {
    let statuses: Vec<(String, PaymentStatus)> = october_plan()
        .series
        .into_iter()
        .map(|row| (row.merchant_key, row.status))
        .collect();

    assert_eq!(
        statuses,
        [
            ("ACME PAYROLL".to_string(), PaymentStatus::Paid),
            ("RENT PORTAL".to_string(), PaymentStatus::Paid),
            ("COMCAST".to_string(), PaymentStatus::Paid),
            ("NETFLIX".to_string(), PaymentStatus::Paid),
            ("CITY WATER".to_string(), PaymentStatus::PastDue),
            ("STATE FARM".to_string(), PaymentStatus::Upcoming),
        ]
    );
}

#[test]
fn typical_plan_should_flag_only_auto_and_transport_overspent() {
    let targets: Vec<(String, bool)> = october_plan()
        .targets
        .into_iter()
        .map(|row| (row.category, row.overspent))
        .collect();

    assert_eq!(
        targets,
        [
            ("Groceries".to_string(), false),
            ("Auto & Transport".to_string(), true),
            ("Dining & Drinks".to_string(), false),
        ]
    );
}

#[test]
fn typical_plan_should_suggest_only_the_unconfirmed_recurring_merchant() {
    // Spotify recurs in Aug and Sep. Every confirmed series is excluded, and
    // so is the dismissed Gym Club.
    let keys: Vec<String> = october_plan()
        .suggestions
        .into_iter()
        .map(|row| row.merchant_key)
        .collect();

    assert_eq!(keys, ["SPOTIFY"]);
}

// --- The typical fixture's October 2026 transactions (§11.4) ---

#[test]
fn typical_transactions_should_total_spending_including_bills() {
    // Every non-transfer, non-income outflow, net of the Amazon refund:
    // 2,400 + 82.50 + 48 + 452.75 + 67.50 + 225.25 + 60 + 24.25 + 18.
    assert_eq!(october_transactions().spent_total, 3_378.25);
}

#[test]
fn typical_transactions_should_count_the_one_uncategorized_merchant() {
    assert_eq!(october_transactions().uncategorized_count, 1);
}

#[test]
fn typical_transactions_should_pair_the_payment_that_crosses_into_october() {
    // Paid from checking on Sep 30, credited to the Apple Card on Oct 2.
    let view = october_transactions();
    let credit = view
        .transactions
        .iter()
        .find(|row| row.id == "oct-03")
        .expect("the Oct 2 card credit is in the fixture");

    assert_eq!(credit.category, TRANSFER);
}

#[test]
fn typical_transactions_should_pair_the_in_month_card_autopay() {
    let view = october_transactions();
    let categories: Vec<&str> = view
        .transactions
        .iter()
        .filter(|row| row.id == "oct-05" || row.id == "oct-06")
        .map(|row| row.category.as_str())
        .collect();

    assert_eq!(categories, [TRANSFER, TRANSFER]);
}
