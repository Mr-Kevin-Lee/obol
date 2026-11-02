//! The monthly spending plan (spec §13.6, D44): a pure function of one
//! month's categorized transactions, `plan.yaml`, and today's date.
//!
//! **Contract (task 111b), implemented by task 115.** The arithmetic, which
//! must reproduce Simplifi's to the cent:
//!
//! - A **series** matches a non-transfer transaction posted in M with the
//!   same merchant key and a sign that fits its kind: a bill matches
//!   outflows (< 0), income inflows (> 0), so a refund from a bill payee —
//!   or a $0 line — is ordinary category activity. In month M a series
//!   counts at the magnitude of its matches' sum if any posted, else at its
//!   expected amount.
//! - **Status**: Paid if matched. Unmatched: Past due in a month before
//!   today's (never paid); in today's month, Past due once the due day is
//!   before today, else Upcoming; Upcoming in a later month. A due day past
//!   the month's end needs no clamping to the last day: it's past every
//!   possible "today" in that month either way, so it stays Upcoming.
//! - Two series with the same merchant and kind would both match one
//!   transaction and count it twice. That's a configuration error left
//!   unguarded: the app's confirm command replaces any existing series for
//!   a merchant, so it never creates one.
//! - **Income** = income-series values + the signed sum of
//!   `Income`-category transactions not matched to a series.
//! - **Bills** = bill-series values. Series matches are never also category
//!   spending.
//! - **Spent** per category = −Σ amount over that category's transactions,
//!   excluding transfers, series matches and `Income`. Refunds reduce it,
//!   and can make it negative.
//! - **Planned** = Σ over targeted categories of max(target, spent); a
//!   target with no activity contributes its full amount. Targets on
//!   `Income` or `Transfer` are ignored.
//! - **Other** = Σ spent over untargeted categories, `Uncategorized`
//!   included.
//! - **Left** = Income − Bills − Planned − Other.
//! - **Per day** = Left ÷ days remaining after today, current month only;
//!   `None` on its last day and for any other month.
//!
//! Sums must start from `+0.0`, not use `Iterator::sum`, whose `f64`
//! identity is `-0.0` and renders as "-$0.00" (found in Phase V).
//!
//! **Suggestions**: non-transfer transactions grouped by merchant key. A
//! merchant qualifies if it appears in at least 2 distinct months, always
//! with the same strict sign (all < 0 or all > 0), with an amount spread
//! (max − min magnitude) of at most 20% of the median magnitude and a
//! due-day spread (max − min day of month) of at most 7. Days don't wrap
//! across a month boundary, so a bill drifting between the 30th and the
//! 2nd reads as a 28-day spread and doesn't qualify. Confirmed and
//! dismissed merchants are excluded. Negative amounts suggest a bill,
//! positive ones income.

use std::cmp::Ordering;
use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use time::Date;

use crate::categorize::{CategorizedTransaction, INCOME, TRANSFER};
use crate::transaction::{Transaction, YearMonth};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SeriesKind {
    Bill,
    Income,
}

/// One confirmed bill or income source.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecurringSeries {
    pub merchant_key: String,
    pub kind: SeriesKind,
    /// A positive magnitude, whatever the kind.
    pub expected_amount: f64,
    /// 1–31.
    pub due_day: u8,
}

/// `plan.yaml`'s contents.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct PlanConfig {
    /// Category → monthly target, a positive amount.
    #[serde(default)]
    pub targets: BTreeMap<String, f64>,
    #[serde(default)]
    pub series: Vec<RecurringSeries>,
    /// Merchant keys whose suggestion was dismissed.
    #[serde(default)]
    pub dismissed: BTreeSet<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PaymentStatus {
    Paid,
    Upcoming,
    PastDue,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SeriesStatus {
    pub series: RecurringSeries,
    /// The actual total if paid this month, else the expected amount; a
    /// positive magnitude.
    pub amount: f64,
    pub status: PaymentStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CategorySpend {
    pub category: String,
    /// Net outflow, positive; a month of refunds can make it negative.
    pub spent: f64,
    pub target: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct MonthPlan {
    pub income: f64,
    pub bills: f64,
    pub planned: f64,
    pub other: f64,
    pub left: f64,
    pub per_day: Option<f64>,
    /// Every configured series, in `plan.yaml` order.
    pub series: Vec<SeriesStatus>,
    /// Every category with spending or a target, most spent first, ties by
    /// name.
    pub categories: Vec<CategorySpend>,
}

/// §13.6's plan for `month`. `transactions` may span more than `month`;
/// only `month`'s count.
pub fn compute_month_plan(
    month: YearMonth,
    transactions: &[CategorizedTransaction],
    plan: &PlanConfig,
    today: Date,
) -> MonthPlan {
    let in_month: Vec<&CategorizedTransaction> = transactions
        .iter()
        .filter(|t| month.contains(t.transaction.posted) && !t.is_transfer())
        .collect();
    let is_series_match = |t: &CategorizedTransaction| plan.series.iter().any(|s| matches(s, t));

    let series: Vec<SeriesStatus> = plan
        .series
        .iter()
        .map(|s| {
            let paid: Vec<f64> = in_month
                .iter()
                .filter(|t| matches(s, t))
                .map(|t| t.transaction.amount)
                .collect();
            let (amount, status) = if paid.is_empty() {
                (s.expected_amount, unpaid_status(s.due_day, month, today))
            } else {
                (total(paid).abs(), PaymentStatus::Paid)
            };
            SeriesStatus {
                series: s.clone(),
                amount,
                status,
            }
        })
        .collect();

    let mut spent: BTreeMap<&str, f64> = plan
        .targets
        .keys()
        .filter(|category| *category != INCOME && *category != TRANSFER)
        .map(|category| (category.as_str(), 0.0))
        .collect();
    for t in in_month
        .iter()
        .filter(|t| t.category != INCOME && !is_series_match(t))
    {
        let category_spent = spent.entry(t.category.as_str()).or_insert(0.0);
        *category_spent -= t.transaction.amount;
    }
    let mut categories: Vec<CategorySpend> = spent
        .into_iter()
        .map(|(category, spent)| CategorySpend {
            category: category.to_string(),
            spent,
            target: plan.targets.get(category).copied(),
        })
        .collect();
    // Stable over the name-ordered map, so equal spends stay in name order.
    categories.sort_by(|a, b| b.spent.total_cmp(&a.spent));

    let series_total = |kind: SeriesKind| {
        total(
            series
                .iter()
                .filter(|s| s.series.kind == kind)
                .map(|s| s.amount),
        )
    };
    let income = series_total(SeriesKind::Income)
        + total(
            in_month
                .iter()
                .filter(|t| t.category == INCOME && !is_series_match(t))
                .map(|t| t.transaction.amount),
        );
    let bills = series_total(SeriesKind::Bill);
    let planned = total(
        categories
            .iter()
            .filter_map(|c| c.target.map(|target| target.max(c.spent))),
    );
    let other = total(
        categories
            .iter()
            .filter(|c| c.target.is_none())
            .map(|c| c.spent),
    );
    let left = income - bills - planned - other;
    let per_day = (YearMonth::of(today) == month)
        .then(|| month.last_day().day() - today.day())
        .filter(|&remaining| remaining > 0)
        .map(|remaining| left / f64::from(remaining));

    MonthPlan {
        income,
        bills,
        planned,
        other,
        left,
        per_day,
        series,
        categories,
    }
}

/// §13.6's suggestions from as much history as the caller has, sorted by
/// merchant key. Expected amount is the median magnitude (the mean of the
/// two middles for an even count); due day is the median day of month (the
/// lower middle for an even count).
pub fn suggest_series(
    history: &[CategorizedTransaction],
    plan: &PlanConfig,
) -> Vec<RecurringSeries> {
    let mut by_merchant: BTreeMap<&str, Vec<&Transaction>> = BTreeMap::new();
    for t in history.iter().filter(|t| !t.is_transfer()) {
        by_merchant
            .entry(t.merchant_key.as_str())
            .or_default()
            .push(&t.transaction);
    }
    by_merchant
        .into_iter()
        .filter(|(merchant, _)| {
            !plan.dismissed.contains(*merchant)
                && !plan.series.iter().any(|s| s.merchant_key == *merchant)
        })
        .filter_map(|(merchant, transactions)| suggestion(merchant, &transactions))
        .collect()
}

fn matches(series: &RecurringSeries, t: &CategorizedTransaction) -> bool {
    t.merchant_key == series.merchant_key
        && match series.kind {
            SeriesKind::Bill => t.transaction.amount < 0.0,
            SeriesKind::Income => t.transaction.amount > 0.0,
        }
}

fn unpaid_status(due_day: u8, month: YearMonth, today: Date) -> PaymentStatus {
    let past_due = match month.cmp(&YearMonth::of(today)) {
        Ordering::Less => true,
        Ordering::Equal => due_day < today.day(),
        Ordering::Greater => false,
    };
    if past_due {
        PaymentStatus::PastDue
    } else {
        PaymentStatus::Upcoming
    }
}

/// `Iterator::sum` would start from `-0.0`; see the module doc.
fn total(values: impl IntoIterator<Item = f64>) -> f64 {
    values.into_iter().fold(0.0, |sum, value| sum + value)
}

/// `transactions` is one merchant's, never empty.
fn suggestion(merchant: &str, transactions: &[&Transaction]) -> Option<RecurringSeries> {
    let kind = if transactions.iter().all(|t| t.amount < 0.0) {
        SeriesKind::Bill
    } else if transactions.iter().all(|t| t.amount > 0.0) {
        SeriesKind::Income
    } else {
        return None;
    };
    let months: BTreeSet<YearMonth> = transactions
        .iter()
        .map(|t| YearMonth::of(t.posted))
        .collect();
    let mut magnitudes: Vec<f64> = transactions.iter().map(|t| t.amount.abs()).collect();
    magnitudes.sort_by(f64::total_cmp);
    let mut days: Vec<u8> = transactions.iter().map(|t| t.posted.day()).collect();
    days.sort_unstable();

    let expected_amount = median(&magnitudes);
    let amount_spread = magnitudes[magnitudes.len() - 1] - magnitudes[0];
    let day_spread = days[days.len() - 1] - days[0];
    (months.len() >= 2 && amount_spread <= 0.2 * expected_amount && day_spread <= 7).then(|| {
        RecurringSeries {
            merchant_key: merchant.to_string(),
            kind,
            expected_amount,
            due_day: days[(days.len() - 1) / 2],
        }
    })
}

/// `sorted` is ascending and non-empty.
fn median(sorted: &[f64]) -> f64 {
    let mid = sorted.len() / 2;
    if sorted.len().is_multiple_of(2) {
        (sorted[mid - 1] + sorted[mid]) / 2.0
    } else {
        sorted[mid]
    }
}

#[cfg(test)]
mod tests {
    use time::Month;

    use super::*;
    use crate::categorize::UNCATEGORIZED;

    const GROCERIES: &str = "Groceries";
    const DINING: &str = "Dining";

    fn october() -> YearMonth {
        YearMonth::new(2026, 10).unwrap()
    }

    fn date(month: Month, day: u8) -> Date {
        Date::from_calendar_date(2026, month, day).unwrap()
    }

    fn oct(day: u8) -> Date {
        date(Month::October, day)
    }

    /// Mid-October, so October is the current month.
    fn today() -> Date {
        oct(15)
    }

    fn tx(merchant: &str, category: &str, posted: Date, amount: f64) -> CategorizedTransaction {
        CategorizedTransaction {
            transaction: Transaction {
                source_id: "chase_checking".into(),
                id: format!("{merchant}-{posted}-{amount}"),
                posted,
                amount,
                description: merchant.into(),
            },
            merchant_key: merchant.into(),
            category: category.into(),
        }
    }

    fn series(
        merchant: &str,
        kind: SeriesKind,
        expected_amount: f64,
        due_day: u8,
    ) -> RecurringSeries {
        RecurringSeries {
            merchant_key: merchant.into(),
            kind,
            expected_amount,
            due_day,
        }
    }

    fn bill(merchant: &str, expected_amount: f64, due_day: u8) -> RecurringSeries {
        series(merchant, SeriesKind::Bill, expected_amount, due_day)
    }

    fn paycheck(expected_amount: f64) -> RecurringSeries {
        series("ACME PAYROLL", SeriesKind::Income, expected_amount, 1)
    }

    fn with_series(series: Vec<RecurringSeries>) -> PlanConfig {
        PlanConfig {
            series,
            ..PlanConfig::default()
        }
    }

    fn with_targets(targets: &[(&str, f64)]) -> PlanConfig {
        PlanConfig {
            targets: targets
                .iter()
                .map(|(category, target)| (category.to_string(), *target))
                .collect(),
            ..PlanConfig::default()
        }
    }

    fn plan_for(transactions: &[CategorizedTransaction], plan: &PlanConfig) -> MonthPlan {
        compute_month_plan(october(), transactions, plan, today())
    }

    fn status_of(month: YearMonth, due_day: u8, today: Date) -> PaymentStatus {
        compute_month_plan(
            month,
            &[],
            &with_series(vec![bill("NETFLIX", 15.0, due_day)]),
            today,
        )
        .series[0]
            .status
    }

    // --- Income ---

    #[test]
    fn should_count_income_category_transactions_as_income() {
        let transactions = [tx("ACME PAYROLL", INCOME, oct(1), 1000.0)];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.income, 1000.0);
    }

    #[test]
    fn should_sum_income_category_transactions_with_their_sign() {
        let transactions = [
            tx("ACME PAYROLL", INCOME, oct(1), 1000.0),
            tx("ACME PAYROLL", INCOME, oct(2), -100.0),
        ];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.income, 900.0);
    }

    #[test]
    fn should_count_an_unmatched_income_series_at_its_expected_amount() {
        let plan = plan_for(&[], &with_series(vec![paycheck(2000.0)]));

        assert_eq!(plan.income, 2000.0);
    }

    #[test]
    fn should_count_a_matched_income_series_once_at_its_actual_amount() {
        let transactions = [tx("ACME PAYROLL", INCOME, oct(1), 2100.0)];

        let plan = plan_for(&transactions, &with_series(vec![paycheck(2000.0)]));

        assert_eq!(plan.income, 2100.0);
    }

    #[test]
    fn should_add_unmatched_income_transactions_to_income_series() {
        let transactions = [tx("SIDE GIG", INCOME, oct(3), 300.0)];

        let plan = plan_for(&transactions, &with_series(vec![paycheck(2000.0)]));

        assert_eq!(plan.income, 2300.0);
    }

    #[test]
    fn should_not_count_bill_series_as_income() {
        let plan = plan_for(&[], &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.income, 0.0);
    }

    #[test]
    fn should_not_list_income_as_a_category() {
        let transactions = [tx("ACME PAYROLL", INCOME, oct(1), 1000.0)];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.categories, []);
    }

    // --- Bills and series matching ---

    #[test]
    fn should_count_an_unpaid_bill_at_its_expected_amount() {
        let plan = plan_for(&[], &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.bills, 15.0);
    }

    #[test]
    fn should_count_a_paid_bill_at_its_actual_magnitude() {
        let transactions = [tx("NETFLIX", "Entertainment", oct(12), -17.5)];

        let plan = plan_for(&transactions, &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.bills, 17.5);
    }

    #[test]
    fn should_sum_every_match_of_one_series() {
        let transactions = [
            tx("NETFLIX", "Entertainment", oct(3), -10.0),
            tx("NETFLIX", "Entertainment", oct(9), -5.0),
        ];

        let plan = plan_for(&transactions, &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.series[0].amount, 15.0);
    }

    #[test]
    fn should_not_count_income_series_as_bills() {
        let plan = plan_for(&[], &with_series(vec![paycheck(2000.0)]));

        assert_eq!(plan.bills, 0.0);
    }

    #[test]
    fn should_mark_a_matched_series_paid() {
        let transactions = [tx("NETFLIX", "Entertainment", oct(12), -15.0)];

        let plan = plan_for(&transactions, &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.series[0].status, PaymentStatus::Paid);
    }

    #[test]
    fn should_mark_a_matched_series_paid_in_a_past_month() {
        let transactions = [tx(
            "NETFLIX",
            "Entertainment",
            date(Month::September, 12),
            -15.0,
        )];
        let september = YearMonth::new(2026, 9).unwrap();

        let plan = compute_month_plan(
            september,
            &transactions,
            &with_series(vec![bill("NETFLIX", 15.0, 20)]),
            today(),
        );

        assert_eq!(plan.series[0].status, PaymentStatus::Paid);
    }

    #[test]
    fn should_not_count_a_series_match_as_category_spending() {
        let transactions = [tx("NETFLIX", "Entertainment", oct(12), -15.0)];

        let plan = plan_for(&transactions, &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.categories, []);
    }

    #[test]
    fn should_not_match_a_refund_from_a_bill_payee() {
        let transactions = [tx("NETFLIX", "Entertainment", oct(12), 15.0)];

        let plan = plan_for(&transactions, &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.series[0].status, PaymentStatus::Upcoming);
    }

    #[test]
    fn should_count_a_bill_payee_refund_as_category_activity() {
        let transactions = [tx("NETFLIX", "Entertainment", oct(12), 15.0)];

        let plan = plan_for(&transactions, &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.other, -15.0);
    }

    #[test]
    fn should_not_match_an_outflow_to_an_income_series() {
        let transactions = [tx("ACME PAYROLL", "Fees", oct(12), -50.0)];
        let mut income = paycheck(2000.0);
        income.due_day = 20;

        let plan = plan_for(&transactions, &with_series(vec![income]));

        assert_eq!(plan.series[0].status, PaymentStatus::Upcoming);
    }

    #[test]
    fn should_not_match_a_zero_amount_transaction_to_a_bill() {
        let transactions = [tx("NETFLIX", "Entertainment", oct(12), 0.0)];

        let plan = plan_for(&transactions, &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.series[0].status, PaymentStatus::Upcoming);
    }

    #[test]
    fn should_not_match_a_zero_amount_transaction_to_income() {
        let transactions = [tx("ACME PAYROLL", INCOME, oct(12), 0.0)];
        let mut income = paycheck(2000.0);
        income.due_day = 20;

        let plan = plan_for(&transactions, &with_series(vec![income]));

        assert_eq!(plan.series[0].status, PaymentStatus::Upcoming);
    }

    #[test]
    fn should_not_match_a_different_merchant() {
        let transactions = [tx("HULU", "Entertainment", oct(12), -15.0)];

        let plan = plan_for(&transactions, &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.series[0].status, PaymentStatus::Upcoming);
    }

    #[test]
    fn should_not_match_a_transaction_from_another_month() {
        let transactions = [tx(
            "NETFLIX",
            "Entertainment",
            date(Month::September, 30),
            -15.0,
        )];

        let plan = plan_for(&transactions, &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.series[0].status, PaymentStatus::Upcoming);
    }

    #[test]
    fn should_list_series_in_plan_order() {
        let plan = plan_for(
            &[],
            &with_series(vec![bill("ZILLOW RENT", 2000.0, 1), bill("ATT", 80.0, 5)]),
        );

        let keys: Vec<&str> = plan
            .series
            .iter()
            .map(|s| s.series.merchant_key.as_str())
            .collect();
        assert_eq!(keys, ["ZILLOW RENT", "ATT"]);
    }

    // --- Status ---

    #[test]
    fn should_mark_an_unpaid_bill_in_a_past_month_past_due() {
        let september = YearMonth::new(2026, 9).unwrap();

        assert_eq!(status_of(september, 28, today()), PaymentStatus::PastDue);
    }

    #[test]
    fn should_mark_an_unpaid_bill_due_before_today_past_due() {
        assert_eq!(status_of(october(), 14, today()), PaymentStatus::PastDue);
    }

    #[test]
    fn should_mark_an_unpaid_bill_due_today_upcoming() {
        assert_eq!(status_of(october(), 15, today()), PaymentStatus::Upcoming);
    }

    #[test]
    fn should_mark_an_unpaid_bill_due_after_today_upcoming() {
        assert_eq!(status_of(october(), 16, today()), PaymentStatus::Upcoming);
    }

    #[test]
    fn should_mark_an_unpaid_bill_in_a_future_month_upcoming() {
        let november = YearMonth::new(2026, 11).unwrap();

        assert_eq!(status_of(november, 1, today()), PaymentStatus::Upcoming);
    }

    #[test]
    fn should_keep_a_day_31_bill_upcoming_on_the_last_day_of_a_30_day_month() {
        let november = YearMonth::new(2026, 11).unwrap();

        assert_eq!(
            status_of(november, 31, date(Month::November, 30)),
            PaymentStatus::Upcoming
        );
    }

    // --- Spent, Planned, Other ---

    #[test]
    fn should_count_untargeted_spending_as_other() {
        let transactions = [tx("SAFEWAY", GROCERIES, oct(3), -80.0)];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.other, 80.0);
    }

    #[test]
    fn should_count_uncategorized_spending_as_other() {
        let transactions = [tx("MYSTERY", UNCATEGORIZED, oct(3), -42.0)];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.other, 42.0);
    }

    #[test]
    fn should_not_count_targeted_spending_as_other() {
        let transactions = [tx("SAFEWAY", GROCERIES, oct(3), -80.0)];

        let plan = plan_for(&transactions, &with_targets(&[(GROCERIES, 300.0)]));

        assert_eq!(plan.other, 0.0);
    }

    #[test]
    fn should_reduce_spent_by_refunds() {
        let transactions = [
            tx("SAFEWAY", GROCERIES, oct(3), -100.0),
            tx("SAFEWAY", GROCERIES, oct(4), 30.0),
        ];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.categories[0].spent, 70.0);
    }

    #[test]
    fn should_let_a_month_of_refunds_make_spent_negative() {
        let transactions = [tx("SAFEWAY", GROCERIES, oct(4), 30.0)];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.categories[0].spent, -30.0);
    }

    #[test]
    fn should_plan_the_full_target_when_a_category_has_no_activity() {
        let plan = plan_for(&[], &with_targets(&[(GROCERIES, 300.0)]));

        assert_eq!(plan.planned, 300.0);
    }

    #[test]
    fn should_plan_the_target_when_spent_is_under_it() {
        let transactions = [tx("SAFEWAY", GROCERIES, oct(3), -100.0)];

        let plan = plan_for(&transactions, &with_targets(&[(GROCERIES, 300.0)]));

        assert_eq!(plan.planned, 300.0);
    }

    #[test]
    fn should_plan_the_spent_amount_when_it_exceeds_the_target() {
        let transactions = [tx("SAFEWAY", GROCERIES, oct(3), -350.0)];

        let plan = plan_for(&transactions, &with_targets(&[(GROCERIES, 300.0)]));

        assert_eq!(plan.planned, 350.0);
    }

    #[test]
    fn should_not_plan_untargeted_spending() {
        let transactions = [tx("SAFEWAY", GROCERIES, oct(3), -100.0)];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.planned, 0.0);
    }

    #[test]
    fn should_ignore_a_target_on_income() {
        let plan = plan_for(&[], &with_targets(&[(INCOME, 500.0)]));

        assert_eq!(plan.planned, 0.0);
    }

    #[test]
    fn should_ignore_a_target_on_transfer() {
        let plan = plan_for(&[], &with_targets(&[(TRANSFER, 500.0)]));

        assert_eq!(plan.planned, 0.0);
    }

    #[test]
    fn should_list_a_target_with_no_activity_as_a_category() {
        let plan = plan_for(&[], &with_targets(&[(GROCERIES, 300.0)]));

        assert_eq!(
            plan.categories,
            [CategorySpend {
                category: GROCERIES.into(),
                spent: 0.0,
                target: Some(300.0),
            }]
        );
    }

    #[test]
    fn should_leave_target_unset_on_an_untargeted_category() {
        let transactions = [tx("SAFEWAY", GROCERIES, oct(3), -100.0)];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.categories[0].target, None);
    }

    #[test]
    fn should_ignore_transactions_outside_the_month() {
        let transactions = [
            tx("SAFEWAY", GROCERIES, date(Month::September, 30), -100.0),
            tx("SAFEWAY", GROCERIES, date(Month::November, 1), -100.0),
        ];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.categories, []);
    }

    #[test]
    fn should_order_categories_by_most_spent_first() {
        // Spend order is the reverse of name order, so only sorting passes.
        let transactions = [
            tx("CHIPOTLE", DINING, oct(3), -50.0),
            tx("SAFEWAY", GROCERIES, oct(3), -100.0),
        ];

        let plan = plan_for(&transactions, &PlanConfig::default());

        let names: Vec<&str> = plan
            .categories
            .iter()
            .map(|c| c.category.as_str())
            .collect();
        assert_eq!(names, [GROCERIES, DINING]);
    }

    #[test]
    fn should_order_categories_with_equal_spent_by_name() {
        // Listed out of name order, so only an explicit name ordering passes.
        let transactions = [
            tx("SHELL", "Gas", oct(3), -50.0),
            tx("CHIPOTLE", DINING, oct(3), -50.0),
            tx("AMC", "Entertainment", oct(3), -50.0),
        ];

        let plan = plan_for(&transactions, &PlanConfig::default());

        let names: Vec<&str> = plan
            .categories
            .iter()
            .map(|c| c.category.as_str())
            .collect();
        assert_eq!(names, [DINING, "Entertainment", "Gas"]);
    }

    // --- Transfers ---

    #[test]
    fn should_not_match_a_transfer_to_a_series() {
        let transactions = [tx("NETFLIX", TRANSFER, oct(12), -15.0)];

        let plan = plan_for(&transactions, &with_series(vec![bill("NETFLIX", 15.0, 20)]));

        assert_eq!(plan.series[0].status, PaymentStatus::Upcoming);
    }

    #[test]
    fn should_not_list_transfers_as_a_category() {
        let transactions = [tx("CHASE CARD PAYMENT", TRANSFER, oct(12), -900.0)];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.categories, []);
    }

    #[test]
    fn should_not_count_transfers_as_other() {
        let transactions = [tx("CHASE CARD PAYMENT", TRANSFER, oct(12), -900.0)];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.other, 0.0);
    }

    // --- Left and per day ---

    #[test]
    fn should_subtract_bills_planned_and_other_from_income() {
        let transactions = [
            tx("ACME PAYROLL", INCOME, oct(1), 1000.0),
            tx("MYSTERY", UNCATEGORIZED, oct(3), -100.0),
        ];
        let plan = PlanConfig {
            series: vec![bill("NETFLIX", 200.0, 20)],
            ..with_targets(&[(GROCERIES, 300.0)])
        };

        let month = plan_for(&transactions, &plan);

        assert_eq!(month.left, 400.0);
    }

    #[test]
    fn should_spread_left_over_the_days_remaining_after_today() {
        // 16 days remain after October 15.
        let transactions = [tx("ACME PAYROLL", INCOME, oct(1), 160.0)];

        let plan = plan_for(&transactions, &PlanConfig::default());

        assert_eq!(plan.per_day, Some(10.0));
    }

    #[test]
    fn should_omit_per_day_on_the_last_day_of_the_month() {
        let transactions = [tx("ACME PAYROLL", INCOME, oct(1), 160.0)];

        let plan = compute_month_plan(october(), &transactions, &PlanConfig::default(), oct(31));

        assert_eq!(plan.per_day, None);
    }

    #[test]
    fn should_omit_per_day_for_a_past_month() {
        let september = YearMonth::new(2026, 9).unwrap();

        let plan = compute_month_plan(september, &[], &PlanConfig::default(), today());

        assert_eq!(plan.per_day, None);
    }

    #[test]
    fn should_omit_per_day_for_a_future_month() {
        let november = YearMonth::new(2026, 11).unwrap();

        let plan = compute_month_plan(november, &[], &PlanConfig::default(), today());

        assert_eq!(plan.per_day, None);
    }

    #[test]
    fn should_report_an_empty_month_as_positive_zeros() {
        let plan = plan_for(&[], &PlanConfig::default());

        let figures = [plan.income, plan.bills, plan.planned, plan.other, plan.left];
        assert_eq!(figures.map(f64::is_sign_positive), [true; 5]);
    }

    fn simplifi_october() -> MonthPlan {
        let transactions = [
            tx("ACME PAYROLL", INCOME, oct(1), 4430.81),
            tx("MYSTERY", UNCATEGORIZED, oct(3), -2545.98),
        ];
        let plan = PlanConfig {
            series: vec![bill("ZILLOW RENT", 6301.89, 28)],
            ..with_targets(&[(GROCERIES, 3186.96)])
        };
        // October 26: five days remain.
        compute_month_plan(october(), &transactions, &plan, oct(26))
    }

    #[test]
    fn should_reproduce_simplifis_left_this_month() {
        assert!((simplifi_october().left - -7604.02).abs() < 0.005);
    }

    #[test]
    fn should_reproduce_simplifis_per_day() {
        assert!((simplifi_october().per_day.unwrap() - -1520.804).abs() < 0.005);
    }

    // --- Suggestions ---

    fn suggest(history: &[CategorizedTransaction]) -> Vec<RecurringSeries> {
        suggest_series(history, &PlanConfig::default())
    }

    fn monthly(merchant: &str, amounts_and_days: &[(f64, u8)]) -> Vec<CategorizedTransaction> {
        amounts_and_days
            .iter()
            .enumerate()
            .map(|(i, &(amount, day))| {
                let month = Month::try_from(i as u8 + 1).unwrap();
                tx(merchant, "Bills", date(month, day), amount)
            })
            .collect()
    }

    #[test]
    fn should_suggest_a_monthly_bill() {
        let history = monthly("NETFLIX", &[(-15.0, 12), (-15.0, 12)]);

        assert_eq!(suggest(&history), [bill("NETFLIX", 15.0, 12)]);
    }

    #[test]
    fn should_suggest_income_for_inflows() {
        let history = monthly("ACME PAYROLL", &[(2000.0, 1), (2000.0, 1)]);

        assert_eq!(
            suggest(&history),
            [series("ACME PAYROLL", SeriesKind::Income, 2000.0, 1)]
        );
    }

    #[test]
    fn should_not_suggest_a_merchant_seen_in_one_month() {
        let history = [
            tx("NETFLIX", "Bills", oct(1), -15.0),
            tx("NETFLIX", "Bills", oct(2), -15.0),
        ];

        assert_eq!(suggest(&history), []);
    }

    #[test]
    fn should_not_suggest_a_merchant_with_mixed_signs() {
        let history = monthly("NETFLIX", &[(-15.0, 12), (15.0, 12)]);

        assert_eq!(suggest(&history), []);
    }

    #[test]
    fn should_not_suggest_a_merchant_whose_amounts_are_all_zero() {
        let history = monthly("NETFLIX", &[(0.0, 12), (0.0, 12)]);

        assert_eq!(suggest(&history), []);
    }

    #[test]
    fn should_suggest_an_amount_spread_of_exactly_20_percent() {
        let history = monthly("ATT", &[(-80.0, 5), (-100.0, 5), (-100.0, 5)]);

        assert_eq!(suggest(&history).len(), 1);
    }

    #[test]
    fn should_not_suggest_an_amount_spread_beyond_20_percent() {
        let history = monthly("ATT", &[(-79.99, 5), (-100.0, 5), (-100.0, 5)]);

        assert_eq!(suggest(&history), []);
    }

    #[test]
    fn should_suggest_a_due_day_spread_of_exactly_7_days() {
        let history = monthly("ATT", &[(-80.0, 1), (-80.0, 8)]);

        assert_eq!(suggest(&history).len(), 1);
    }

    #[test]
    fn should_not_suggest_a_due_day_spread_beyond_7_days() {
        let history = monthly("ATT", &[(-80.0, 1), (-80.0, 9)]);

        assert_eq!(suggest(&history), []);
    }

    #[test]
    fn should_never_suggest_transfers() {
        let history: Vec<_> = monthly("CHASE CARD PAYMENT", &[(-900.0, 3), (-900.0, 3)])
            .into_iter()
            .map(|t| CategorizedTransaction {
                category: TRANSFER.into(),
                ..t
            })
            .collect();

        assert_eq!(suggest(&history), []);
    }

    #[test]
    fn should_not_suggest_a_dismissed_merchant() {
        let history = monthly("NETFLIX", &[(-15.0, 12), (-15.0, 12)]);
        let plan = PlanConfig {
            dismissed: ["NETFLIX".to_string()].into(),
            ..PlanConfig::default()
        };

        assert_eq!(suggest_series(&history, &plan), []);
    }

    #[test]
    fn should_not_suggest_an_already_confirmed_merchant() {
        let history = monthly("NETFLIX", &[(-15.0, 12), (-15.0, 12)]);
        let plan = with_series(vec![bill("NETFLIX", 15.0, 12)]);

        assert_eq!(suggest_series(&history, &plan), []);
    }

    #[test]
    fn should_expect_the_median_magnitude_of_an_odd_count() {
        let history = monthly("ATT", &[(-110.0, 5), (-100.0, 5), (-104.0, 5)]);

        assert_eq!(suggest(&history)[0].expected_amount, 104.0);
    }

    #[test]
    fn should_expect_the_mean_of_the_middle_magnitudes_of_an_even_count() {
        let history = monthly("ATT", &[(-110.0, 5), (-100.0, 5), (-106.0, 5), (-104.0, 5)]);

        assert_eq!(suggest(&history)[0].expected_amount, 105.0);
    }

    #[test]
    fn should_use_the_median_due_day_of_an_odd_count() {
        let history = monthly("ATT", &[(-80.0, 9), (-80.0, 3), (-80.0, 5)]);

        assert_eq!(suggest(&history)[0].due_day, 5);
    }

    #[test]
    fn should_use_the_lower_middle_due_day_of_an_even_count() {
        let history = monthly("ATT", &[(-80.0, 9), (-80.0, 3), (-80.0, 8), (-80.0, 5)]);

        assert_eq!(suggest(&history)[0].due_day, 5);
    }

    #[test]
    fn should_sort_suggestions_by_merchant_key() {
        let mut history = monthly("ZILLOW RENT", &[(-2000.0, 1), (-2000.0, 1)]);
        history.extend(monthly("ATT", &[(-80.0, 5), (-80.0, 5)]));

        let keys: Vec<String> = suggest(&history)
            .into_iter()
            .map(|s| s.merchant_key)
            .collect();
        assert_eq!(keys, ["ATT", "ZILLOW RENT"]);
    }
}
