/** Calendar-day buckets use local midnights (including daylight-saving changes). */
export function groupHistoryDays<T extends { timestampMs: number }>(entries: readonly T[], now = new Date()) {
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime()
  const yesterday = new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1).getTime()
  const groups = new Map<number, { label: string; entries: T[] }>()
  for (const entry of [...entries].sort((a, b) => b.timestampMs - a.timestampMs)) {
    const date = new Date(entry.timestampMs)
    const key = new Date(date.getFullYear(), date.getMonth(), date.getDate()).getTime()
    const label = key === today ? 'TODAY' : key === yesterday ? 'YESTERDAY' : date.toLocaleDateString('en-US', {
      weekday: 'long', month: 'long', day: 'numeric', year: date.getFullYear() === now.getFullYear() ? undefined : 'numeric',
    }).toUpperCase()
    if (!groups.has(key)) groups.set(key, { label, entries: [] })
    groups.get(key)!.entries.push(entry)
  }
  return [...groups.values()]
}
