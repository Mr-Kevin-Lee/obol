// The only date arithmetic the frontend does (D43): which month the
// selector shows, and today's local date for the backend's `today`
// argument, so the backend never has to guess the user's timezone.

const pad = (n: number) => String(n).padStart(2, '0')

/** Today in local time, 'YYYY-MM-DD'. */
export function localToday(): string {
  const now = new Date()
  return `${now.getFullYear()}-${pad(now.getMonth() + 1)}-${pad(now.getDate())}`
}

/** This month in local time, 'YYYY-MM'. */
export function currentMonth(): string {
  return localToday().slice(0, 7)
}

/** 'YYYY-MM' moved by `delta` months, across year boundaries. */
export function shiftMonth(month: string, delta: number): string {
  const [year, m] = month.split('-').map(Number)
  const shifted = new Date(year, m - 1 + delta, 1)
  return `${shifted.getFullYear()}-${pad(shifted.getMonth() + 1)}`
}
