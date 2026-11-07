// Mirrors the serialized view models in crates/desktop/src/view.rs,
// dashboard.rs and budget.rs. Hand-written rather than generated, so keep
// them in step: a field renamed there and not here fails silently at
// runtime.

export type NetWorthView =
  | { kind: 'computed'; value: number }
  | { kind: 'unavailable'; total_sources: number }

export type StatusView = 'ok' | 'error' | 'unknown'

export interface AccountView {
  source_id: string
  institution: string
  account_type: string
  balance: number | null
  currency: string
  status: StatusView
  error_message: string | null
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
  /** Net worth per saved snapshot, oldest first (the sparkline). */
  trend: number[]
}

export interface RefreshResult {
  view: DashboardView
  save_warning: string | null
  /** SimpleFIN accounts this refresh linked on its own, one
   *  "<institution> <name>" label each; empty most of the time. */
  added_sources: string[]
  /** New SimpleFIN transactions this refresh saved. */
  transactions_added: number
  /** Why some older SimpleFIN transactions are still missing; a later
   *  refresh fetches them. */
  sync_warning: string | null
}

// --- SimpleFIN transaction history (sync.rs) ---
//
//   invoke<EarlierResult>('load_earlier_transactions')  // one more request: the 90 days before the earliest day fetched

export interface EarlierResult {
  added: number
  /** 'YYYY-MM-DD': how far back SimpleFIN history now reaches. */
  history_from: string
}

// --- Connecting SimpleFIN (connect.rs) ---
//
//   invoke<ConnectStatus>('connect_status')
//   invoke<ConnectPreview>('preview_simplefin', { setupToken })  // setupToken: string | null (null reuses the saved connection)
//   invoke<LinkResult>('link_simplefin_accounts', { accounts })  // every row, ticked or not; then call 'refresh'
//
// `preview_simplefin` with a token claims it, which works only once: on
// success the token is spent, so the form must not resend it.

export type Side = 'asset' | 'liability'

export interface ConnectAccount {
  account_id: string
  institution: string
  name: string
  /** SimpleFIN's numeric string, e.g. "-120.50". */
  balance: string
  side: Side
  /** Lowercase snake_case: checking, savings, money_market, credit_card,
   *  loan, investment, other. */
  account_type: string
  /** Ticked in the form. Unticked rows are remembered and never added by
   *  Refresh. */
  include: boolean
}

export interface ConnectPreview {
  /** Accounts not linked yet, in the Bridge's order. */
  accounts: ConnectAccount[]
  /** The Bridge's own error messages, e.g. a bank needing sign-in again. */
  warnings: string[]
}

export interface LinkResult {
  linked: number
}

export interface ConnectStatus {
  /** 0 means nothing is connected: offer the connect form. */
  simplefin_sources: number
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
//   invoke<TransactionsView>('set_transaction_excluded', { month, sourceId, id, excluded })
//   invoke<ImportResult>('import_simplifi', { contents })  // the picked file's text; categories only, additive; reload the view after
//   invoke<SpendingPlanView>('get_spending_plan', { month, today })
//   invoke<SpendingPlanView>('set_category_target', { month, today, category, target })  // target: number | null (null removes it)
//   invoke<SpendingPlanView>('confirm_series', { month, today, merchantKey, kind, expectedAmount, dueDay })
//   invoke<SpendingPlanView>('dismiss_series', { month, today, merchantKey })
//   invoke<SpendingPlanView>('remove_series', { month, today, merchantKey })  // also dismisses it
//   invoke<SpendingPlanView>('add_series', { month, today, name, kind, expectedAmount, dueDay })  // name as typed
//
// Editing a confirmed series' amount or due day is `confirm_series` with
// its existing merchantKey and kind: it replaces the series.
//
// `today` is the local date from the webview's clock, so the backend
// never has to guess the user's timezone.

export interface CategoryBar {
  category: string
  spent: number
  percentage: number
}

export interface TransactionRow {
  /** Pays a bill confirmed in the Spending plan, so it isn't discretionary. */
  bill: boolean
  /** Left out of every total, bar and the Spending plan, by hand. */
  excluded: boolean
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
  /** spent_total without payments to confirmed bills. */
  discretionary_spent_total: number
  /** categories without payments to confirmed bills. */
  discretionary_categories: CategoryBar[]
  /** Already sorted, newest first. */
  transactions: TransactionRow[]
  uncategorized_count: number
  /** Display order. Always contains every row's current category,
   *  'Income', 'Transfer' and 'Uncategorized' included. */
  available_categories: string[]
  /** 'YYYY-MM-DD': the earliest day SimpleFIN transactions have been
   *  fetched for; null before the first sync. */
  simplefin_history_from: string | null
}

export interface ImportResult {
  /** Merchant categories learned; the import never adds transactions. */
  rules_added: number
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
