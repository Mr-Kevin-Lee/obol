// Presentation only. Every figure arrives already computed by the Rust
// view model (D43: no domain logic in the frontend); this file just
// decides how it reads.

const moneyFormats = new Map<string, Intl.NumberFormat>()

export function formatMoney(amount: number, currency = 'USD'): string {
  let format = moneyFormats.get(currency)
  if (!format) {
    format = new Intl.NumberFormat('en-US', { style: 'currency', currency })
    moneyFormats.set(currency, format)
  }
  // `-0 === 0`, so this turns negative zero into plain zero. Without it an
  // f64 -0.0 from the backend (e.g. core's net worth over a single paid-off
  // card) renders as "-$0.00".
  return format.format(amount === 0 ? 0 : amount)
}

/** "+$1,112.75" or "−$1,112.75" — a true minus sign, not a hyphen. */
export function formatSignedMoney(amount: number): string {
  const sign = amount > 0 ? '+' : amount < 0 ? '−' : ''
  return sign + formatMoney(Math.abs(amount))
}

const dateFormat = new Intl.DateTimeFormat('en-US', { dateStyle: 'medium' })
const dateTimeFormat = new Intl.DateTimeFormat('en-US', {
  dateStyle: 'medium',
  timeStyle: 'short',
})

export function formatDate(iso: string): string {
  return dateFormat.format(new Date(iso))
}

export function formatDateTime(iso: string): string {
  return dateTimeFormat.format(new Date(iso))
}

// Spec §11's account types. Anything newer falls through to a
// humanized form of the raw value rather than being hidden.
const accountTypeLabels: Record<string, string> = {
  checking: 'Checking',
  money_market: 'Money market',
  brokerage: 'Brokerage',
  retirement_401k: '401(k)',
  '529': '529 plan',
  rsu_stock: 'RSU / stock',
  credit_card: 'Credit card',
  mortgage: 'Mortgage',
  student_loan: 'Student loan',
}

export function accountTypeLabel(type: string): string {
  const known = accountTypeLabels[type]
  if (known) return known
  const spaced = type.replaceAll('_', ' ')
  return spaced.charAt(0).toUpperCase() + spaced.slice(1)
}
