// One way to write dates and times, everywhere in the app (Apple style):
//   just now · 5 min ago · Today 7:25 PM · Yesterday 7:25 PM · Oct 2 · Oct 2, 2025
// Every screen uses these instead of its own toLocale… call.

const DAY_MS = 86_400_000

const valid = (ms: number) => Number.isFinite(ms) && Math.abs(ms) <= 8.64e15

export function startOfDay(ms: number): number {
  const d = new Date(ms)
  d.setHours(0, 0, 0, 0)
  return d.getTime()
}

/** "7:25 PM" (24-hour where the locale says so). */
export function clockTime(ms: number): string {
  if (!valid(ms)) return '—'
  return new Date(ms).toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
}

/** "Oct 2", or "Oct 2, 2025" outside the current year. */
export function shortDate(ms: number): string {
  if (!valid(ms)) return '—'
  const d = new Date(ms)
  return d.toLocaleDateString(undefined, {
    month: 'short',
    day: 'numeric',
    year: d.getFullYear() === new Date().getFullYear() ? undefined : 'numeric',
  })
}

/** "Today" / "Yesterday" / null. */
function nearDay(ms: number): 'Today' | 'Yesterday' | null {
  const today = startOfDay(Date.now())
  const day = startOfDay(ms)
  if (day === today) return 'Today'
  if (day === today - DAY_MS) return 'Yesterday'
  return null
}

/**
 * When something happened, for status lines and lists:
 * "Just now", "5 min ago" (under an hour), "Today 7:25 PM", "Yesterday 7:25 PM",
 * then "Oct 2" ("Oct 2, 2025" in another year).
 */
export function relativeTime(ms: number): string {
  if (!valid(ms)) return '—'
  const sec = Math.floor((Date.now() - ms) / 1000)
  if (sec < 45) return 'Just now'
  const min = Math.floor(sec / 60)
  if (min < 60) return `${Math.max(1, min)} min ago`
  const near = nearDay(ms)
  if (near) return `${near} ${clockTime(ms)}`
  return shortDate(ms)
}

/** A date with its time, for tooltips and file lists: "Today 7:25 PM", "Oct 2, 7:25 PM". */
export function dateTime(ms: number): string {
  if (!valid(ms)) return '—'
  const near = nearDay(ms)
  return near ? `${near} ${clockTime(ms)}` : `${shortDate(ms)}, ${clockTime(ms)}`
}

/** Section heading for a day: "Today", "Yesterday", "Tuesday" (this week), "Oct 2". */
export function dayHeading(ms: number): string {
  if (!valid(ms)) return '—'
  const near = nearDay(ms)
  if (near) return near
  const today = startOfDay(Date.now())
  if (today - startOfDay(ms) < 6 * DAY_MS) return new Date(ms).toLocaleDateString(undefined, { weekday: 'long' })
  return shortDate(ms)
}

/** A compact list column: "7:25 PM" today, then "Yesterday", "Tuesday", "Oct 2". */
export function listDate(ms: number | undefined | null): string {
  if (ms == null || !valid(ms)) return ''
  const near = nearDay(ms)
  if (near === 'Today') return clockTime(ms)
  if (near) return near
  return dayHeading(ms)
}
