// Display formatting helpers. Bytes use decimal units (1000) to match croc.

export function formatBytes(bytes: number, decimals = 1): string {
  if (!bytes || bytes <= 0) return '0 B'
  const k = 1000
  const units = ['B', 'kB', 'MB', 'GB', 'TB', 'PB']
  const i = Math.min(Math.floor(Math.log(bytes) / Math.log(k)), units.length - 1)
  const v = bytes / Math.pow(k, i)
  return `${v.toFixed(i === 0 ? 0 : decimals)} ${units[i]}`
}

// Live transfer counter: keep TWO decimals once we're into GB+ so a slow multi-GB
// transfer visibly ticks (5.11 → 5.22 GB) instead of sitting on a frozen "5.1 GB"
// (GitHub #25). Sub-GB stays at the usual one decimal.
export function formatBytesLive(bytes: number): string {
  if (!bytes || bytes <= 0) return '0 B'
  const i = Math.floor(Math.log(bytes) / Math.log(1000))
  return formatBytes(bytes, i >= 3 ? 2 : 1)
}

// Whether to show speeds in megaBITS/sec (Mbps) vs megaBYTES/sec (MB/s). Set
// once from settings (setSpeedUnit) so every formatSpeed call stays consistent
// without threading the preference through every component.
let SPEED_IN_MEGABITS = false
export function setSpeedUnit(megabits: boolean): void {
  SPEED_IN_MEGABITS = megabits
}

// `bytesPerSec` is BYTES per second (despite the legacy name).
export function formatSpeed(bytesPerSec: number): string {
  if (!bytesPerSec || bytesPerSec <= 0) return '—'
  if (SPEED_IN_MEGABITS) {
    const mbps = (bytesPerSec * 8) / 1_000_000
    return `${mbps.toFixed(mbps < 10 ? 1 : 0)} Mbps`
  }
  return `${formatBytes(bytesPerSec)}/s`
}

export function formatEta(seconds: number | null | undefined): string {
  if (seconds == null || !isFinite(seconds) || seconds < 0) return '—'
  if (seconds < 1) return '<1s'
  const s = Math.round(seconds)
  if (s < 60) return `${s}s`
  const m = Math.floor(s / 60)
  const rs = s % 60
  if (m < 60) return rs ? `${m}m ${rs}s` : `${m}m`
  const h = Math.floor(m / 60)
  const rm = m % 60
  return rm ? `${h}h ${rm}m` : `${h}h`
}

export function formatRelativeTime(ms: number): string {
  const now = Date.now()
  const diff = now - ms
  const sec = Math.floor(diff / 1000)
  if (sec < 45) return 'Just now'
  const min = Math.floor(sec / 60)
  if (min < 60) return `${min}m ago`
  const d = new Date(ms)
  const today = new Date()
  const yesterday = new Date()
  yesterday.setDate(today.getDate() - 1)
  const time = d.toLocaleTimeString(undefined, { hour: 'numeric', minute: '2-digit' })
  if (d.toDateString() === today.toDateString()) return `Today ${time}`
  if (d.toDateString() === yesterday.toDateString()) return `Yesterday ${time}`
  return d.toLocaleDateString(undefined, { month: 'short', day: 'numeric' }) + ` ${time}`
}

export function shortPath(p: string, max = 42): string {
  if (p.length <= max) return p
  const parts = p.split('/')
  if (parts.length <= 2) return '…' + p.slice(-(max - 1))
  const last = parts[parts.length - 1]
  return `${parts[0]}/…/${last}`.length <= max
    ? `${parts[0]}/…/${last}`
    : '…/' + last
}
