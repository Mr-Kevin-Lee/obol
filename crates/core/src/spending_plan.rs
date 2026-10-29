//! The monthly spending plan (spec §13.6, D44): a pure function of one
//! month's categorized transactions, `plan.yaml`, and today's date.
//!
//! **Contract (task 111b), implemented by task 115.** The arithmetic, which
//! must reproduce Simplifi's to the cent:
//!
//! - A **series** matches a transaction on merchant key. In month M it
//!   counts at its actual total if a match posted in M, else at its
//!   expected amount. Status: Paid if matched; else Past due if M is
//!   today's month and the due day is before today; else Upcoming. A due
//!   day past the month's end means its last day.
//! - **Income** = income-series values + `Income`-category transactions not
//!   matched to a series.
//! - **Bills** = bill-series values. Series matches are never also category
//!   spending.
//! - **Spent** per category = −Σ amount over that category's transactions,
//!   excluding transfers, series matches and `Income`. Refunds reduce it.
//! - **Planned** = Σ over targeted categories of max(target, spent).
//! - **Other** = Σ spent over untargeted categories, `Uncategorized`
//!   included.
//! - **Left** = Income − Bills − Planned − Other.
//! - **Per day** = Left ÷ days remaining after today, current month only;
//!   `None` on its last day and for any other month.
//!
//! Sums must start from `+0.0`, not use `Iterator::sum`, whose `f64`
//! identity is `-0.0` and renders as "-$0.00" (found in Phase V).
//!
//! **Suggestions**: a merchant qualifies if it appears in at least 2
//! distinct months, always with the same sign, with an amount spread within
//! 20% and a due-day spread within 7 days. Transfers never qualify;
//! confirmed and dismissed merchants are excluded. Negative amounts suggest
//! a bill, positive ones income.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};
use time::Date;

use crate::categorize::CategorizedTransaction;
use crate::transaction::YearMonth;

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
#[allow(unused_variables)] // stub until task 115
pub fn compute_month_plan(
    month: YearMonth,
    transactions: &[CategorizedTransaction],
    plan: &PlanConfig,
    today: Date,
) -> MonthPlan {
    todo!("task 115")
}

/// §13.6's suggestions from as much history as the caller has, sorted by
/// merchant key. Expected amount is the median magnitude; due day is the
/// median day of month.
#[allow(unused_variables)] // stub until task 115
pub fn suggest_series(
    history: &[CategorizedTransaction],
    plan: &PlanConfig,
) -> Vec<RecurringSeries> {
    todo!("task 115")
}
