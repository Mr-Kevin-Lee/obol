//! The Transactions and Spending plan view models (spec §11.4, §13.6,
//! §14, D44): the shapes the webview renders for those two screens.
//! Mirrored by hand in `ui/src/types.ts` — keep the two in step, since a
//! renamed field fails silently at runtime.
//!
//! **Contract (task 111b).** The structs are final; the two assembly
//! functions are task 116's. Amounts follow core's conventions: a
//! transaction's `amount` is signed (negative is money out), while every
//! plan figure, `spent`, `target` and series amount is a magnitude.

use obol_core::{Date, PaymentStatus, SeriesKind, YearMonth};
use serde::Serialize;

use crate::dashboard::StorageLayout;

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
    /// the bare `source_id` if no snapshot names it.
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

/// Task 116.
#[allow(unused_variables)] // stub until task 116
pub fn transactions_view(
    layout: &StorageLayout,
    month: YearMonth,
) -> Result<TransactionsView, String> {
    todo!("task 116")
}

/// Task 116. `today` comes from the webview — the local date as the user's
/// clock shows it — rather than from Rust, so no timezone handling is
/// needed here.
#[allow(unused_variables)] // stub until task 116
pub fn spending_plan_view(
    layout: &StorageLayout,
    month: YearMonth,
    today: Date,
) -> Result<SpendingPlanView, String> {
    todo!("task 116")
}
