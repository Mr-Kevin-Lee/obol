// Mirrors the serialized view models in crates/desktop/src/view.rs,
// dashboard.rs and budget.rs. Hand-written rather than generated, so keep
// them in step: a field renamed there and not here fails silently at
// runtime.

export type NetWorthView =
  | { kind: 'computed'; value: number }
  | { kind: 'unavailable'; total_sources: number }

export type StatusView = 'ok' | 'error' | 'unknown'

export interface HoldingBucketView {
  label: string
  value: number
  percentage: number
}

export interface AccountView {
  source_id: string
  institution: string
  account_type: string
  balance: number | null
  currency: string
  status: StatusView
  error_message: string | null
  holdings: HoldingBucketView[] | null
}

export interface DeltaView {
  amount: number
  since: string
}

export interface DashboardView {
  created_at: string
  net_worth: NetWorthView
  assets: AccountView[]
  liabilities: AccountView[]
  assets_subtotal: number
  liabilities_subtotal: number
  delta: DeltaView | null
}

export interface RefreshResult {
  view: DashboardView
  save_warning: string | null
}

// --- Transactions and Spending plan (budget.rs, D44) ---
//
// Months are 'YYYY-MM' and dates 'YYYY-MM-DD'. A transaction's `amount` is
// signed (negative is money out); every plan figure, `spent`, `target` and
// series amount is a positive magnitude.
//
// Commands (Tauri maps these camelCase argument names onto the Rust
// handlers' snake_case parameters). Each mutation returns the refreshed
// view, so the screen re-renders from one round trip:
//
//   invoke<TransactionsView>('get_transactions', { month })
//   invoke<TransactionsView>('set_merchant_category', { month, merchantKey, category })
//   invoke<SpendingPlanView>('get_spending_plan', { month, today })
//   invoke<SpendingPlanView>('set_category_target', { month, today, category, target })  // target: number | null (null removes it)
//   invoke<SpendingPlanView>('confirm_series', { month, today, merchantKey, kind, expectedAmount, dueDay })
//   invoke<SpendingPlanView>('dismiss_series', { month, today, merchantKey })
//
// `today` is the local date from the webview's clock, so the backend
// never has to guess the user's timezone.

export interface CategoryBar {
  category: string
  spent: number
  percentage: number
}

export interface TransactionRow {
  source_id: string
  id: string
  posted: string
  merchant_key: string
  merchant_name: string
  account_label: string
  amount: number
  category: string
}

export interface TransactionsView {
  month: string
  spent_total: number
  /** Already sorted, most spent first. */
  categories: CategoryBar[]
  /** Already sorted, newest first. */
  transactions: TransactionRow[]
  uncategorized_count: number
  /** Display order. Always contains every row's current category,
   *  'Income', 'Transfer' and 'Uncategorized' included. */
  available_categories: string[]
}

export type SeriesKind = 'bill' | 'income'

export type PaymentStatus = 'paid' | 'upcoming' | 'past_due'

export interface SeriesRow {
  merchant_key: string
  name: string
  kind: SeriesKind
  amount: number
  expected_amount: number
  due_day: number
  status: PaymentStatus
}

export interface TargetRow {
  category: string
  spent: number
  target: number
  overspent: boolean
  /** spent as a percentage of target; over 100 when overspent — cap the
   *  bar width and use `overspent` for the text cue. */
  percentage: number
}

export interface SuggestionRow {
  merchant_key: string
  name: string
  kind: SeriesKind
  expected_amount: number
  due_day: number
}

export interface SpendingPlanView {
  month: string
  is_current_month: boolean
  income: number
  bills: number
  planned: number
  other: number
  left: number
  /** Bills + Planned + Other as a percentage of Income — the "left this
   *  month" bar; over 100 means overspent. null when Income is zero or less. */
  committed_percentage: number | null
  per_day: number | null
  /** plan.yaml order. */
  series: SeriesRow[]
  /** Already sorted, most spent first. */
  targets: TargetRow[]
  /** Already sorted, most spent first. */
  other_categories: CategoryBar[]
  /** Already sorted by merchant. */
  suggestions: SuggestionRow[]
  /** Display order, excluding 'Income' and 'Transfer' (no spending target). */
  available_categories: string[]
}
