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
  categories: CategoryBar[]
  transactions: TransactionRow[]
  uncategorized_count: number
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
  per_day: number | null
  series: SeriesRow[]
  targets: TargetRow[]
  other_categories: CategoryBar[]
  suggestions: SuggestionRow[]
  available_categories: string[]
}
