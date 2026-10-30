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
  // `new Date('2026-10-05')` is UTC midnight, which is still Oct 4 west of
  // Greenwich; a bare calendar date has to be read as local.
  const day = /^(\d{4})-(\d{2})-(\d{2})$/.exec(iso)
  return dateFormat.format(day ? new Date(+day[1], +day[2] - 1, +day[3]) : new Date(iso))
}

const monthFormat = new Intl.DateTimeFormat('en-US', { month: 'long', year: 'numeric' })

/** 'YYYY-MM' → "October 2026". */
export function formatMonth(month: string): string {
  const [year, m] = month.split('-').map(Number)
  return monthFormat.format(new Date(year, m - 1, 1))
}

const ordinals = new Intl.PluralRules('en-US', { type: 'ordinal' })
const ordinalSuffix: Record<string, string> = { one: 'st', two: 'nd', few: 'rd', other: 'th' }

/** 15 → "15th". */
export function formatOrdinal(n: number): string {
  return `${n}${ordinalSuffix[ordinals.select(n)]}`
}

/** A percentage as a CSS bar width: an overspend fills the track rather
 *  than overflowing it, and a negative one (net refunds) stays empty. */
export function barWidth(p: number): string {
  return `${Math.min(Math.max(p, 0), 100)}%`
}

/** 61.234 → "61.2%". */
export function formatPercentage(p: number): string {
  return `${p.toFixed(1)}%`
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
