// Mirrors the serialized view model in crates/desktop/src/view.rs and
// dashboard.rs. Hand-written rather than generated, so keep the two in
// step: a field renamed there and not here fails silently at runtime.

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
