//! The Transactions and Spending plan view models (spec §11.4, §13.6,
//! §14, D44): the shapes the webview renders for those two screens.
//! Mirrored by hand in `ui/src/types.ts` — keep the two in step, since a
//! renamed field fails silently at runtime.
//!
//! **Contract (task 111b).** The structs are final; the two assembly
//! functions are task 116's. Amounts follow core's conventions: a
//! transaction's `amount` is signed (negative is money out), while every
//! plan figure, `spent`, `target` and series amount is a magnitude.

use std::collections::{BTreeMap, HashMap};
use std::path::Path;

use obol_core::{
    available_categories, categorize, compute_month_plan, display_name, format_iso_date,
    import_simplifi_export, load_or_init_merchant_rules, load_or_init_plan, load_recent_snapshots,
    load_transactions_range, save_plan, set_merchant_category, suggest_series,
    CategorizedTransaction, Date, MerchantRules, PaymentStatus, PlanConfig, RecurringSeries,
    SeriesKind, YearMonth, INCOME, TRANSFER, UNCATEGORIZED,
};
use serde::Serialize;

use crate::dashboard::StorageLayout;

/// How much history series suggestions look at: the viewed month and the
/// eleven before it.
const SUGGESTION_HISTORY_MONTHS: usize = 12;

/// One bar in a category breakdown.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CategoryBar {
    pub category: String,
    pub spent: f64,
    /// Of the breakdown's total spent, 0–100.
    pub percentage: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransactionRow {
    pub source_id: String,
    pub id: String,
    /// `YYYY-MM-DD`.
    pub posted: String,
    pub merchant_key: String,
    pub merchant_name: String,
    /// The source's institution and account type, from the latest snapshot;
    /// "<account> (Simplifi)" for an imported one, else the bare `source_id`.
    pub account_label: String,
    /// Signed: negative is money out.
    pub amount: f64,
    pub category: String,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TransactionsView {
    /// `YYYY-MM`.
    pub month: String,
    /// Net spending, excluding transfers and income.
    pub spent_total: f64,
    /// Spending categories, most spent first.
    pub categories: Vec<CategoryBar>,
    /// Newest first.
    pub transactions: Vec<TransactionRow>,
    pub uncategorized_count: usize,
    /// For the category picker, in display order. Always contains every
    /// row's current category, `Income`, `Transfer` and `Uncategorized`
    /// included, so a picker never lacks the value it's showing.
    pub available_categories: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesRow {
    pub merchant_key: String,
    pub name: String,
    pub kind: SeriesKind,
    /// The actual total if paid, else the expected amount.
    pub amount: f64,
    pub expected_amount: f64,
    pub due_day: u8,
    pub status: PaymentStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct TargetRow {
    pub category: String,
    pub spent: f64,
    pub target: f64,
    pub overspent: bool,
    /// `spent` as a percentage of `target`; over 100 when overspent, so
    /// the UI caps the bar width and relies on `overspent` for the text.
    pub percentage: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SuggestionRow {
    pub merchant_key: String,
    pub name: String,
    pub kind: SeriesKind,
    pub expected_amount: f64,
    pub due_day: u8,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SpendingPlanView {
    /// `YYYY-MM`.
    pub month: String,
    pub is_current_month: bool,
    pub income: f64,
    pub bills: f64,
    pub planned: f64,
    pub other: f64,
    pub left: f64,
    /// Bills + Planned + Other as a percentage of Income — the "left this
    /// month" bar. Over 100 means overspent. `None` when Income is zero or
    /// less, where a percentage of it means nothing.
    pub committed_percentage: Option<f64>,
    /// Current month only, and not on its last day.
    pub per_day: Option<f64>,
    pub series: Vec<SeriesRow>,
    /// Categories with a target, most spent first.
    pub targets: Vec<TargetRow>,
    /// Categories without a target, most spent first.
    pub other_categories: Vec<CategoryBar>,
    pub suggestions: Vec<SuggestionRow>,
    /// For adding a target to a category, in display order. Excludes
    /// `Income` and `Transfer`, which can't carry a spending target.
    pub available_categories: Vec<String>,
}

pub fn transactions_view(
    layout: &StorageLayout,
    month: YearMonth,
) -> Result<TransactionsView, String> {
    let rules = read_rules(layout)?;
    // The adjacent months are read too so transfer pairing sees both sides
    // of a payment that straddles a month boundary (§11.4).
    let categorized = categorized_window(layout, month.previous(), month.next(), &rules)?;
    let in_month: Vec<&CategorizedTransaction> = categorized
        .iter()
        .filter(|transaction| month.contains(transaction.transaction.posted))
        .collect();
    let labels = account_labels(layout);

    let spend = spend_by_category(in_month.iter().copied());
    let spent_total = spend.values().fold(0.0, |total, spent| total + spent);

    Ok(TransactionsView {
        month: month.to_string(),
        spent_total,
        categories: category_bars(spend),
        transactions: in_month
            .iter()
            .map(|transaction| transaction_row(transaction, &labels))
            .collect(),
        uncategorized_count: in_month
            .iter()
            .filter(|transaction| transaction.category == UNCATEGORIZED)
            .count(),
        available_categories: available_categories(&rules),
    })
}

/// `today` comes from the webview — the local date as the user's clock
/// shows it — rather than from Rust, so no timezone handling is needed
/// here.
pub fn spending_plan_view(
    layout: &StorageLayout,
    month: YearMonth,
    today: Date,
) -> Result<SpendingPlanView, String> {
    let rules = read_rules(layout)?;
    let plan = read_plan(layout)?;
    let earliest = (1..SUGGESTION_HISTORY_MONTHS).fold(month, |earlier, _| earlier.previous());
    let categorized = categorized_window(layout, earliest, month.next(), &rules)?;

    let month_plan = compute_month_plan(month, &categorized, &plan, today);
    let (targeted, untargeted): (Vec<_>, Vec<_>) = month_plan
        .categories
        .iter()
        .partition(|category| category.target.is_some());

    Ok(SpendingPlanView {
        month: month.to_string(),
        is_current_month: month == YearMonth::of(today),
        income: month_plan.income,
        bills: month_plan.bills,
        planned: month_plan.planned,
        other: month_plan.other,
        left: month_plan.left,
        committed_percentage: (month_plan.income > 0.0).then(|| {
            (month_plan.bills + month_plan.planned + month_plan.other) / month_plan.income * 100.0
        }),
        per_day: month_plan.per_day,
        series: month_plan
            .series
            .iter()
            .map(|status| SeriesRow {
                merchant_key: status.series.merchant_key.clone(),
                name: display_name(&status.series.merchant_key),
                kind: status.series.kind,
                amount: status.amount,
                expected_amount: status.series.expected_amount,
                due_day: status.series.due_day,
                status: status.status,
            })
            .collect(),
        targets: targeted
            .iter()
            .filter_map(|category| {
                let target = category.target?;
                Some(TargetRow {
                    category: category.category.clone(),
                    spent: category.spent,
                    target,
                    overspent: category.spent > target,
                    // Targets set through the app are always positive; a
                    // hand-edited zero would otherwise divide to infinity,
                    // which serializes as `null` and breaks the bar.
                    percentage: if target > 0.0 {
                        category.spent / target * 100.0
                    } else {
                        0.0
                    },
                })
            })
            .collect(),
        other_categories: category_bars(
            untargeted
                .iter()
                .map(|category| (category.category.clone(), category.spent)),
        ),
        suggestions: suggest_series(&categorized, &plan)
            .into_iter()
            .map(|series| SuggestionRow {
                name: display_name(&series.merchant_key),
                merchant_key: series.merchant_key,
                kind: series.kind,
                expected_amount: series.expected_amount,
                due_day: series.due_day,
            })
            .collect(),
        available_categories: available_categories(&rules)
            .into_iter()
            .filter(|category| category != INCOME && category != TRANSFER)
            .collect(),
    })
}

/// Points a merchant at a category; every past and future transaction from
/// it follows (§11.4). `merchant_key` is taken as given — callers pass one
/// from a view row, already normalized.
pub fn set_category(
    layout: &StorageLayout,
    merchant_key: &str,
    category: &str,
) -> Result<(), String> {
    let merchant_key = non_empty(merchant_key, "merchant")?;
    let category = non_empty(category, "category")?;
    set_merchant_category(&layout.merchant_rules, merchant_key, category)
        .map(|_| ())
        .map_err(|err| err.to_string())
}

/// Sets a category's monthly target, or removes it when `target` is `None`.
pub fn set_target(
    layout: &StorageLayout,
    category: &str,
    target: Option<f64>,
) -> Result<(), String> {
    let category = non_empty(category, "category")?;
    if category == INCOME || category == TRANSFER {
        return Err(format!("{category} can't have a spending target"));
    }
    if let Some(target) = target {
        positive(target, "a target")?;
    }

    update_plan(layout, |plan| match target {
        Some(target) => {
            plan.targets.insert(category.to_string(), target);
        }
        None => {
            plan.targets.remove(category);
        }
    })
}

/// Confirms a bill or income series, replacing any existing series for the
/// same merchant — so the app can never create the duplicate the plan
/// computation treats as a configuration error — and clearing any earlier
/// dismissal of it.
pub fn confirm_series(layout: &StorageLayout, series: RecurringSeries) -> Result<(), String> {
    non_empty(&series.merchant_key, "merchant")?;
    positive(series.expected_amount, "an expected amount")?;
    if !(1..=31).contains(&series.due_day) {
        return Err(format!(
            "due day {} is not between 1 and 31",
            series.due_day
        ));
    }

    update_plan(layout, |plan| {
        plan.series
            .retain(|existing| existing.merchant_key != series.merchant_key);
        plan.dismissed.remove(&series.merchant_key);
        plan.series.push(series);
    })
}

/// Dismisses a suggestion so it isn't offered again.
pub fn dismiss_series(layout: &StorageLayout, merchant_key: &str) -> Result<(), String> {
    let merchant_key = non_empty(merchant_key, "merchant")?;
    update_plan(layout, |plan| {
        plan.dismissed.insert(merchant_key.to_string());
    })
}

/// What a Simplifi import added. Imports are additive, so nothing else
/// can have changed.
#[derive(Debug, Clone, Copy, PartialEq, Serialize)]
pub struct ImportResult {
    pub added: usize,
    pub rules_added: usize,
}

/// The Simplifi import (task 121): `path` is the export CSV.
pub fn import_simplifi(layout: &StorageLayout, path: &str) -> Result<ImportResult, String> {
    let path = non_empty(path, "path")?;
    let summary = import_simplifi_export(
        Path::new(path),
        &layout.transactions,
        &layout.merchant_rules,
    )
    .map_err(|err| err.to_string())?;
    Ok(ImportResult {
        added: summary.transactions.added,
        rules_added: summary.rules_added,
    })
}

// The two views only ever read. `load_or_init_*` writes an empty file when
// one is missing; a read doing that could race an edit in flight and land
// its empty file after the edit's, wiping it. So a missing file reads as
// empty here, and only the mutations ever create one.

fn read_rules(layout: &StorageLayout) -> Result<MerchantRules, String> {
    if !layout.merchant_rules.exists() {
        return Ok(MerchantRules::default());
    }
    load_or_init_merchant_rules(&layout.merchant_rules).map_err(|err| err.to_string())
}

fn read_plan(layout: &StorageLayout) -> Result<PlanConfig, String> {
    if !layout.plan.exists() {
        return Ok(PlanConfig::default());
    }
    load_or_init_plan(&layout.plan).map_err(|err| err.to_string())
}

fn update_plan(layout: &StorageLayout, change: impl FnOnce(&mut PlanConfig)) -> Result<(), String> {
    let mut plan = load_or_init_plan(&layout.plan).map_err(|err| err.to_string())?;
    change(&mut plan);
    save_plan(&layout.plan, &plan).map_err(|err| err.to_string())
}

fn categorized_window(
    layout: &StorageLayout,
    from: YearMonth,
    to: YearMonth,
    rules: &MerchantRules,
) -> Result<Vec<CategorizedTransaction>, String> {
    let transactions = load_transactions_range(&layout.transactions, from, to)
        .map_err(|err| format!("transactions could not be read: {err}"))?;
    Ok(categorize(&transactions, rules))
}

/// Source id → "Institution account type", from the latest snapshot.
/// Labels are a nicety, so unreadable history degrades to bare source ids
/// rather than failing the screen.
fn account_labels(layout: &StorageLayout) -> HashMap<String, String> {
    load_recent_snapshots(&layout.snapshots, 1)
        .unwrap_or_default()
        .into_iter()
        .next()
        .map(|snapshot| {
            snapshot
                .accounts
                .iter()
                .map(|record| {
                    (
                        record.source_id().to_string(),
                        format!(
                            "{} {}",
                            record.institution(),
                            record.account_type().replace('_', " ")
                        ),
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Net outflow per category, leaving out transfers and income. Unlike the
/// spending plan, bill payments stay in their categories: this screen
/// shows where money went, not what was planned.
fn spend_by_category<'a>(
    transactions: impl Iterator<Item = &'a CategorizedTransaction>,
) -> BTreeMap<String, f64> {
    let mut spend = BTreeMap::new();
    for transaction in transactions {
        if transaction.category == TRANSFER || transaction.category == INCOME {
            continue;
        }
        *spend.entry(transaction.category.clone()).or_insert(0.0) -= transaction.transaction.amount;
    }
    spend
}

/// Bars for every category with positive spending, most first, ties by
/// name; a category that's net refunded has nothing to draw.
fn category_bars(spend: impl IntoIterator<Item = (String, f64)>) -> Vec<CategoryBar> {
    let mut positive: Vec<(String, f64)> = spend
        .into_iter()
        .filter(|(_, spent)| *spent > 0.0)
        .collect();
    // Every entry is positive, so a non-empty list has a positive total.
    let total = positive.iter().fold(0.0, |total, (_, spent)| total + spent);
    positive.sort_by(|a, b| b.1.total_cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    positive
        .into_iter()
        .map(|(category, spent)| CategoryBar {
            percentage: spent / total * 100.0,
            category,
            spent,
        })
        .collect()
}

fn transaction_row(
    transaction: &CategorizedTransaction,
    labels: &HashMap<String, String>,
) -> TransactionRow {
    let source_id = &transaction.transaction.source_id;
    TransactionRow {
        source_id: source_id.clone(),
        id: transaction.transaction.id.clone(),
        posted: format_iso_date(transaction.transaction.posted),
        merchant_key: transaction.merchant_key.clone(),
        merchant_name: display_name(&transaction.merchant_key),
        account_label: labels.get(source_id).cloned().unwrap_or_else(|| {
            // Imported accounts have no snapshot to name them.
            match source_id.strip_prefix("simplifi:") {
                Some(account) => format!("{account} (Simplifi)"),
                None => source_id.clone(),
            }
        }),
        amount: transaction.transaction.amount,
        category: transaction.category.clone(),
    }
}

fn non_empty<'a>(value: &'a str, what: &str) -> Result<&'a str, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Err(format!("a {what} can't be empty"));
    }
    Ok(trimmed)
}

fn positive(value: f64, what: &str) -> Result<(), String> {
    if value.is_finite() && value > 0.0 {
        Ok(())
    } else {
        Err(format!("{what} must be a positive amount"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{asset, liability, snapshot_at};
    use obol_core::{load_or_init_merchant_rules, load_or_init_plan, parse_iso_date};
    use serde_json::{json, Value};
    use std::fs;
    use std::path::PathBuf;

    fn temp_layout(name: &str) -> StorageLayout {
        let root: PathBuf = std::env::temp_dir().join(format!(
            "obol-desktop-budget-test-{name}-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&root);
        fs::create_dir_all(&root).unwrap();
        StorageLayout::under(&root)
    }

    fn october() -> YearMonth {
        YearMonth::new(2026, 10).unwrap()
    }

    fn today() -> Date {
        parse_iso_date("2026-10-26").unwrap()
    }

    fn tx(source_id: &str, id: &str, posted: &str, amount: f64, description: &str) -> Value {
        json!({
            "source_id": source_id,
            "id": id,
            "posted": posted,
            "amount": amount,
            "description": description,
        })
    }

    /// Writes month files directly rather than through `save_transactions`,
    /// so these tests exercise this module's reads, not storage's upsert.
    fn write_transactions(layout: &StorageLayout, transactions: Vec<Value>) {
        fs::create_dir_all(&layout.transactions).unwrap();
        let mut by_month: std::collections::BTreeMap<String, Vec<Value>> = Default::default();
        for transaction in transactions {
            let month = transaction["posted"].as_str().unwrap()[..7].to_string();
            by_month.entry(month).or_default().push(transaction);
        }
        for (month, rows) in by_month {
            fs::write(
                layout.transactions.join(format!("{month}.json")),
                serde_json::to_string(&rows).unwrap(),
            )
            .unwrap();
        }
    }

    fn write_rules(layout: &StorageLayout, rules: &[(&str, &str)]) {
        let body: String = rules
            .iter()
            .map(|(key, category)| format!("  {key}: {category}\n"))
            .collect();
        fs::write(&layout.merchant_rules, format!("rules:\n{body}")).unwrap();
    }

    fn write_plan(layout: &StorageLayout, yaml: &str) {
        fs::write(&layout.plan, yaml).unwrap();
    }

    fn series(
        merchant_key: &str,
        kind: SeriesKind,
        expected_amount: f64,
        due_day: u8,
    ) -> RecurringSeries {
        RecurringSeries {
            merchant_key: merchant_key.into(),
            kind,
            expected_amount,
            due_day,
        }
    }

    // --- transactions_view ---

    #[test]
    fn should_label_accounts_from_the_latest_snapshot() {
        let layout = temp_layout("label-snapshot");
        obol_core::save_snapshot(
            &layout.snapshots,
            &snapshot_at(
                "2026-10-20T09:00:00Z",
                json!([asset("chase_checking", 1000.0)]),
            ),
        )
        .unwrap();
        write_transactions(
            &layout,
            vec![tx("chase_checking", "t1", "2026-10-05", -10.0, "AMAZON")],
        );

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(view.transactions[0].account_label, "Chase checking");
    }

    #[test]
    fn should_humanize_the_account_type_in_labels() {
        let layout = temp_layout("label-humanized");
        obol_core::save_snapshot(
            &layout.snapshots,
            &snapshot_at(
                "2026-10-20T09:00:00Z",
                json!([liability("chase_sapphire", 300.0)]),
            ),
        )
        .unwrap();
        write_transactions(
            &layout,
            vec![tx("chase_sapphire", "t1", "2026-10-05", -10.0, "AMAZON")],
        );

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(
            view.transactions[0].account_label,
            "Goldman Sachs credit card"
        );
    }

    #[test]
    fn should_fall_back_to_the_source_id_without_a_snapshot() {
        let layout = temp_layout("label-fallback");
        write_transactions(
            &layout,
            vec![tx("chase_checking", "t1", "2026-10-05", -10.0, "AMAZON")],
        );

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(view.transactions[0].account_label, "chase_checking");
    }

    #[test]
    fn should_list_only_the_months_transactions_newest_first() {
        let layout = temp_layout("month-only");
        write_transactions(
            &layout,
            vec![
                tx("a", "sep30", "2026-09-30", -1.0, "AMAZON"),
                tx("a", "oct01", "2026-10-01", -2.0, "AMAZON"),
                tx("a", "oct15", "2026-10-15", -3.0, "AMAZON"),
                tx("a", "nov01", "2026-11-01", -4.0, "AMAZON"),
            ],
        );

        let view = transactions_view(&layout, october()).unwrap();

        let ids: Vec<&str> = view
            .transactions
            .iter()
            .map(|row| row.id.as_str())
            .collect();
        assert_eq!(ids, ["oct15", "oct01"]);
    }

    #[test]
    fn should_leave_transfers_and_income_out_of_spending() {
        let layout = temp_layout("spend-excludes");
        write_rules(
            &layout,
            &[("ACME PAYROLL", "Income"), ("AMAZON", "Shopping")],
        );
        write_transactions(
            &layout,
            vec![
                tx("checking", "pay", "2026-10-01", 4500.0, "ACME PAYROLL"),
                tx(
                    "checking",
                    "out",
                    "2026-10-05",
                    -100.0,
                    "CHASE CARD AUTOPAY",
                ),
                tx("card", "in", "2026-10-06", 100.0, "PAYMENT RECEIVED"),
                tx("card", "buy", "2026-10-07", -50.0, "AMAZON"),
            ],
        );

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(view.spent_total, 50.0);
    }

    #[test]
    fn should_count_bill_payments_as_category_spending() {
        // The Transactions screen shows every outflow by category; only the
        // spending plan moves series matches into Bills.
        let layout = temp_layout("spend-bills");
        write_rules(&layout, &[("NETFLIX", "Entertainment")]);
        write_plan(
            &layout,
            "series:\n  - merchant_key: NETFLIX\n    kind: bill\n    expected_amount: 15.5\n    due_day: 20\n",
        );
        write_transactions(
            &layout,
            vec![tx("card", "n", "2026-10-20", -15.5, "NETFLIX")],
        );

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(view.spent_total, 15.5);
    }

    #[test]
    fn should_pair_a_transfer_across_a_month_boundary() {
        // A payment on the 30th and its credit on the 2nd only pair if the
        // adjacent month is read too.
        let layout = temp_layout("pair-boundary");
        write_transactions(
            &layout,
            vec![
                tx(
                    "checking",
                    "out",
                    "2026-09-30",
                    -300.0,
                    "APPLE CARD PAYMENT",
                ),
                tx("applecard", "in", "2026-10-02", 300.0, "PAYMENT RECEIVED"),
            ],
        );

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(view.transactions[0].category, obol_core::TRANSFER);
    }

    #[test]
    fn should_sort_category_bars_by_spend_with_percentages() {
        let layout = temp_layout("bars");
        write_rules(&layout, &[("COSTCO", "Groceries"), ("AMAZON", "Shopping")]);
        write_transactions(
            &layout,
            vec![
                tx("card", "g", "2026-10-03", -25.0, "COSTCO"),
                tx("card", "s", "2026-10-04", -75.0, "AMAZON"),
            ],
        );

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(
            view.categories,
            [
                CategoryBar {
                    category: "Shopping".into(),
                    spent: 75.0,
                    percentage: 75.0
                },
                CategoryBar {
                    category: "Groceries".into(),
                    spent: 25.0,
                    percentage: 25.0
                },
            ]
        );
    }

    #[test]
    fn should_break_category_bar_ties_by_name() {
        let layout = temp_layout("bars-tie");
        write_rules(&layout, &[("COSTCO", "Groceries"), ("AMAZON", "Shopping")]);
        write_transactions(
            &layout,
            vec![
                tx("card", "s", "2026-10-03", -10.0, "AMAZON"),
                tx("card", "g", "2026-10-04", -10.0, "COSTCO"),
            ],
        );

        let view = transactions_view(&layout, october()).unwrap();

        let categories: Vec<&str> = view
            .categories
            .iter()
            .map(|bar| bar.category.as_str())
            .collect();
        assert_eq!(categories, ["Groceries", "Shopping"]);
    }

    #[test]
    fn should_leave_a_zero_spend_category_out_of_the_bars() {
        // A purchase fully refunded nets to exactly nothing: no bar to draw.
        let layout = temp_layout("bars-zero");
        write_rules(&layout, &[("AMAZON", "Shopping"), ("COSTCO", "Groceries")]);
        write_transactions(
            &layout,
            vec![
                tx("card", "buy", "2026-10-03", -10.0, "AMAZON"),
                tx("card", "refund", "2026-10-04", 10.0, "AMAZON"),
                tx("card", "g", "2026-10-05", -5.0, "COSTCO"),
            ],
        );

        let view = transactions_view(&layout, october()).unwrap();

        let categories: Vec<&str> = view
            .categories
            .iter()
            .map(|bar| bar.category.as_str())
            .collect();
        assert_eq!(categories, ["Groceries"]);
    }

    #[test]
    fn should_leave_a_net_refund_category_out_of_the_bars() {
        let layout = temp_layout("bars-refund");
        write_rules(&layout, &[("AMAZON", "Shopping"), ("COSTCO", "Groceries")]);
        write_transactions(
            &layout,
            vec![
                tx("card", "buy", "2026-10-03", -20.0, "AMAZON"),
                tx("card", "refund", "2026-10-04", 50.0, "AMAZON"),
                tx("card", "g", "2026-10-05", -40.0, "COSTCO"),
            ],
        );

        let view = transactions_view(&layout, october()).unwrap();

        let categories: Vec<&str> = view
            .categories
            .iter()
            .map(|bar| bar.category.as_str())
            .collect();
        assert_eq!(categories, ["Groceries"]);
    }

    #[test]
    fn should_net_refunds_in_the_spent_total() {
        let layout = temp_layout("total-refund");
        write_rules(&layout, &[("AMAZON", "Shopping"), ("COSTCO", "Groceries")]);
        write_transactions(
            &layout,
            vec![
                tx("card", "buy", "2026-10-03", -20.0, "AMAZON"),
                tx("card", "refund", "2026-10-04", 50.0, "AMAZON"),
                tx("card", "g", "2026-10-05", -40.0, "COSTCO"),
            ],
        );

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(view.spent_total, 10.0);
    }

    #[test]
    fn should_report_zero_percentages_when_nothing_was_spent() {
        // Same guard as the holdings bars: no NaN widths.
        let layout = temp_layout("bars-empty");
        write_transactions(&layout, vec![]);

        let view = transactions_view(&layout, october()).unwrap();

        assert!(view.spent_total.is_sign_positive() && view.categories.is_empty());
    }

    #[test]
    fn should_count_uncategorized_transactions() {
        let layout = temp_layout("uncategorized");
        write_rules(&layout, &[("AMAZON", "Shopping")]);
        write_transactions(
            &layout,
            vec![
                tx("card", "a", "2026-10-03", -1.0, "AMAZON"),
                tx("card", "b", "2026-10-04", -2.0, "CORNER BODEGA"),
                tx("card", "c", "2026-10-05", -3.0, "FARMERS STALL"),
            ],
        );

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(view.uncategorized_count, 2);
    }

    #[test]
    fn should_name_merchants_for_display() {
        let layout = temp_layout("display-name");
        write_transactions(
            &layout,
            vec![tx("card", "a", "2026-10-03", -1.0, "UBER EATS")],
        );

        let view = transactions_view(&layout, october()).unwrap();

        let row = &view.transactions[0];
        assert_eq!(
            row.merchant_name,
            obol_core::display_name(&row.merchant_key)
        );
    }

    #[test]
    fn should_report_the_month_it_shows() {
        let layout = temp_layout("month-label");

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(view.month, "2026-10");
    }

    // --- spending_plan_view ---

    #[test]
    fn should_mark_the_current_month() {
        let layout = temp_layout("current-month");

        let (current, previous) = (
            spending_plan_view(&layout, october(), today()).unwrap(),
            spending_plan_view(&layout, october().previous(), today()).unwrap(),
        );

        assert_eq!(
            (current.is_current_month, previous.is_current_month),
            (true, false)
        );
    }

    #[test]
    fn should_split_categories_into_targets_and_other() {
        let layout = temp_layout("targets-split");
        write_rules(&layout, &[("COSTCO", "Groceries"), ("AMAZON", "Shopping")]);
        write_plan(&layout, "targets:\n  Groceries: 600.0\n");
        write_transactions(
            &layout,
            vec![
                tx("card", "g", "2026-10-03", -50.0, "COSTCO"),
                tx("card", "s", "2026-10-04", -30.0, "AMAZON"),
            ],
        );

        let view = spending_plan_view(&layout, october(), today()).unwrap();

        let split: (Vec<&str>, Vec<&str>) = (
            view.targets
                .iter()
                .map(|row| row.category.as_str())
                .collect(),
            view.other_categories
                .iter()
                .map(|bar| bar.category.as_str())
                .collect(),
        );
        assert_eq!(split, (vec!["Groceries"], vec!["Shopping"]));
    }

    #[test]
    fn should_flag_an_overspent_target_with_its_percentage() {
        let layout = temp_layout("target-over");
        write_rules(&layout, &[("SHELL", "Auto & Transport")]);
        write_plan(&layout, "targets:\n  Auto & Transport: 100.0\n");
        write_transactions(
            &layout,
            vec![tx("card", "s", "2026-10-03", -150.0, "SHELL")],
        );

        let view = spending_plan_view(&layout, october(), today()).unwrap();

        let row = &view.targets[0];
        assert_eq!((row.overspent, row.percentage), (true, 150.0));
    }

    #[test]
    fn should_not_flag_a_target_spent_exactly() {
        let layout = temp_layout("target-exact");
        write_rules(&layout, &[("SHELL", "Auto & Transport")]);
        write_plan(&layout, "targets:\n  Auto & Transport: 100.0\n");
        write_transactions(
            &layout,
            vec![tx("card", "s", "2026-10-03", -100.0, "SHELL")],
        );

        let view = spending_plan_view(&layout, october(), today()).unwrap();

        assert!(!view.targets[0].overspent);
    }

    #[test]
    fn should_compute_committed_percentage_of_income() {
        let layout = temp_layout("committed");
        write_rules(
            &layout,
            &[("ACME PAYROLL", "Income"), ("AMAZON", "Shopping")],
        );
        write_transactions(
            &layout,
            vec![
                tx("checking", "pay", "2026-10-01", 1000.0, "ACME PAYROLL"),
                tx("card", "s", "2026-10-04", -250.0, "AMAZON"),
            ],
        );

        let view = spending_plan_view(&layout, october(), today()).unwrap();

        assert_eq!(view.committed_percentage, Some(25.0));
    }

    #[test]
    fn should_omit_committed_percentage_without_income() {
        let layout = temp_layout("committed-none");
        write_rules(&layout, &[("AMAZON", "Shopping")]);
        write_transactions(
            &layout,
            vec![tx("card", "s", "2026-10-04", -250.0, "AMAZON")],
        );

        let view = spending_plan_view(&layout, october(), today()).unwrap();

        assert_eq!(view.committed_percentage, None);
    }

    #[test]
    fn should_offer_no_targets_on_income_or_transfer() {
        let layout = temp_layout("target-categories");

        let view = spending_plan_view(&layout, october(), today()).unwrap();

        assert!(view
            .available_categories
            .iter()
            .all(|category| category != obol_core::INCOME && category != obol_core::TRANSFER));
    }

    #[test]
    fn should_suggest_series_from_earlier_months() {
        let layout = temp_layout("suggest");
        write_transactions(
            &layout,
            vec![
                tx("card", "aug", "2026-08-27", -10.99, "SPOTIFY"),
                tx("card", "sep", "2026-09-27", -10.99, "SPOTIFY"),
            ],
        );

        let view = spending_plan_view(&layout, october(), today()).unwrap();

        let keys: Vec<&str> = view
            .suggestions
            .iter()
            .map(|row| row.merchant_key.as_str())
            .collect();
        assert_eq!(keys, ["SPOTIFY"]);
    }

    #[test]
    fn should_name_series_for_display() {
        let layout = temp_layout("series-name");
        write_plan(
            &layout,
            "series:\n  - merchant_key: STATE FARM\n    kind: bill\n    expected_amount: 125.0\n    due_day: 28\n",
        );

        let view = spending_plan_view(&layout, october(), today()).unwrap();

        assert_eq!(view.series[0].name, obol_core::display_name("STATE FARM"));
    }

    #[test]
    fn should_report_zero_percentage_for_a_hand_edited_zero_target() {
        // The app never sets one, but plan.yaml is a plain file; dividing by
        // it would produce infinity, which serializes as null.
        let layout = temp_layout("target-zero");
        write_rules(&layout, &[("SHELL", "Auto & Transport")]);
        write_plan(&layout, "targets:\n  Auto & Transport: 0.0\n");
        write_transactions(&layout, vec![tx("card", "s", "2026-10-03", -50.0, "SHELL")]);

        let view = spending_plan_view(&layout, october(), today()).unwrap();

        assert_eq!(view.targets[0].percentage, 0.0);
    }

    #[test]
    fn should_create_no_files_when_viewing() {
        // A view that created files could race an edit in flight and
        // overwrite it with an empty file.
        let layout = temp_layout("views-read-only");

        transactions_view(&layout, october()).unwrap();
        spending_plan_view(&layout, october(), today()).unwrap();

        assert_eq!(
            (layout.merchant_rules.exists(), layout.plan.exists()),
            (false, false)
        );
    }

    // --- mutations ---

    #[test]
    fn should_reject_an_empty_merchant_key() {
        let layout = temp_layout("set-category-empty-key");

        assert!(set_category(&layout, " ", "Groceries").is_err());
    }

    #[test]
    fn should_write_a_merchant_rule() {
        let layout = temp_layout("set-category");

        set_category(&layout, "CORNER BODEGA", "Groceries").unwrap();

        let rules = load_or_init_merchant_rules(&layout.merchant_rules).unwrap();
        assert_eq!(
            rules.rules.get("CORNER BODEGA").map(String::as_str),
            Some("Groceries")
        );
    }

    #[test]
    fn should_reject_an_empty_category() {
        let layout = temp_layout("set-category-empty");

        assert!(set_category(&layout, "CORNER BODEGA", "  ").is_err());
    }

    #[test]
    fn should_set_a_target() {
        let layout = temp_layout("target-set");

        set_target(&layout, "Groceries", Some(600.0)).unwrap();

        assert_eq!(
            load_or_init_plan(&layout.plan)
                .unwrap()
                .targets
                .get("Groceries"),
            Some(&600.0)
        );
    }

    #[test]
    fn should_replace_a_target() {
        let layout = temp_layout("target-replace");
        set_target(&layout, "Groceries", Some(600.0)).unwrap();

        set_target(&layout, "Groceries", Some(450.0)).unwrap();

        assert_eq!(
            load_or_init_plan(&layout.plan)
                .unwrap()
                .targets
                .get("Groceries"),
            Some(&450.0)
        );
    }

    #[test]
    fn should_remove_a_target_when_cleared() {
        let layout = temp_layout("target-clear");
        set_target(&layout, "Groceries", Some(600.0)).unwrap();

        set_target(&layout, "Groceries", None).unwrap();

        assert!(load_or_init_plan(&layout.plan).unwrap().targets.is_empty());
    }

    #[test]
    fn should_reject_a_non_positive_target() {
        let layout = temp_layout("target-non-positive");

        assert!(
            set_target(&layout, "Groceries", Some(0.0)).is_err()
                && set_target(&layout, "Groceries", Some(-5.0)).is_err()
        );
    }

    #[test]
    fn should_reject_a_non_finite_target() {
        let layout = temp_layout("target-non-finite");

        assert!(
            set_target(&layout, "Groceries", Some(f64::NAN)).is_err()
                && set_target(&layout, "Groceries", Some(f64::INFINITY)).is_err()
        );
    }

    #[test]
    fn should_reject_a_target_on_income_or_transfer() {
        let layout = temp_layout("target-income");

        assert!(
            set_target(&layout, obol_core::INCOME, Some(100.0)).is_err()
                && set_target(&layout, obol_core::TRANSFER, Some(100.0)).is_err()
        );
    }

    #[test]
    fn should_confirm_a_series() {
        let layout = temp_layout("confirm");

        confirm_series(&layout, series("SPOTIFY", SeriesKind::Bill, 10.99, 27)).unwrap();

        assert_eq!(
            load_or_init_plan(&layout.plan).unwrap().series,
            [series("SPOTIFY", SeriesKind::Bill, 10.99, 27)]
        );
    }

    #[test]
    fn should_replace_an_existing_series_for_the_same_merchant() {
        let layout = temp_layout("confirm-replace");
        confirm_series(&layout, series("SPOTIFY", SeriesKind::Bill, 10.99, 27)).unwrap();

        confirm_series(&layout, series("SPOTIFY", SeriesKind::Bill, 11.99, 28)).unwrap();

        assert_eq!(
            load_or_init_plan(&layout.plan).unwrap().series,
            [series("SPOTIFY", SeriesKind::Bill, 11.99, 28)]
        );
    }

    #[test]
    fn should_clear_a_dismissal_when_confirming() {
        let layout = temp_layout("confirm-undismiss");
        dismiss_series(&layout, "SPOTIFY").unwrap();

        confirm_series(&layout, series("SPOTIFY", SeriesKind::Bill, 10.99, 27)).unwrap();

        assert!(load_or_init_plan(&layout.plan)
            .unwrap()
            .dismissed
            .is_empty());
    }

    #[test]
    fn should_reject_an_out_of_range_due_day() {
        let layout = temp_layout("confirm-due-day");

        assert!(
            confirm_series(&layout, series("SPOTIFY", SeriesKind::Bill, 10.99, 0)).is_err()
                && confirm_series(&layout, series("SPOTIFY", SeriesKind::Bill, 10.99, 32)).is_err()
        );
    }

    #[test]
    fn should_reject_a_non_positive_expected_amount() {
        let layout = temp_layout("confirm-amount");

        assert!(
            confirm_series(&layout, series("SPOTIFY", SeriesKind::Bill, 0.0, 27)).is_err()
                && confirm_series(&layout, series("SPOTIFY", SeriesKind::Bill, f64::NAN, 27))
                    .is_err()
        );
    }

    #[test]
    fn should_dismiss_a_suggestion() {
        let layout = temp_layout("dismiss");

        dismiss_series(&layout, "SPOTIFY").unwrap();

        assert!(load_or_init_plan(&layout.plan)
            .unwrap()
            .dismissed
            .contains("SPOTIFY"));
    }

    fn write_export(layout: &StorageLayout) -> String {
        let path = layout.transactions.with_file_name("export.csv");
        fs::write(
            &path,
            "Date,Account,Reviewed,Payee,Category,Exclusion,Amount\n\
             \"Oct 20, 2026\",Card,no,Corner Cafe,Dining & Drinks,no, -4.00\n\
             \"Oct 21, 2026\",Card,no,Green Grocer,Groceries,no, -60.00\n",
        )
        .unwrap();
        path.to_string_lossy().into_owned()
    }

    #[test]
    fn should_show_imported_transactions_in_their_month() {
        let layout = temp_layout("import-view");
        let path = write_export(&layout);

        import_simplifi(&layout, &path).unwrap();

        assert_eq!(
            transactions_view(&layout, october())
                .unwrap()
                .transactions
                .len(),
            2
        );
    }

    #[test]
    fn should_categorize_imported_transactions_by_the_seeded_rules() {
        let layout = temp_layout("import-rules");
        let path = write_export(&layout);

        import_simplifi(&layout, &path).unwrap();

        assert_eq!(
            transactions_view(&layout, october()).unwrap().transactions[0].category,
            "Groceries"
        );
    }

    #[test]
    fn should_report_what_the_import_added() {
        let layout = temp_layout("import-counts");
        let path = write_export(&layout);

        let result = import_simplifi(&layout, &path).unwrap();

        assert_eq!(
            result,
            ImportResult {
                added: 2,
                rules_added: 2
            }
        );
    }

    #[test]
    fn should_label_an_imported_account_by_its_simplifi_name() {
        let layout = temp_layout("import-label");
        let path = write_export(&layout);
        import_simplifi(&layout, &path).unwrap();

        let view = transactions_view(&layout, october()).unwrap();

        assert_eq!(view.transactions[0].account_label, "Card (Simplifi)");
    }

    #[test]
    fn should_trim_whitespace_around_the_import_path() {
        let layout = temp_layout("import-trim");
        let path = write_export(&layout);

        assert!(import_simplifi(&layout, &format!("  {path}\n")).is_ok());
    }

    #[test]
    fn should_reject_an_empty_import_path() {
        let layout = temp_layout("import-empty");

        assert_eq!(
            import_simplifi(&layout, "  ").unwrap_err(),
            "a path can't be empty"
        );
    }

    #[test]
    fn should_report_a_missing_export_file() {
        let layout = temp_layout("import-missing");

        let err = import_simplifi(&layout, "/nonexistent/export.csv").unwrap_err();

        assert!(err.contains("could not be read"));
    }
}
