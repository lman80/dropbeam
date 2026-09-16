// Display formatting helpers. Bytes use decimal units (1000) to match croc.

// Above 1 GB this keeps TWO decimals, so a slow multi-GB transfer visibly ticks
// (5.11 GB → 5.12 GB of 56.65 GB) instead of sitting on a frozen "5.1 GB"
// (GitHub #25). Below 1 GB the usual one decimal is plenty. Pass `decimals` to
// override (whole bytes are always shown without a fraction).
export function formatBytes(bytes: number, decimals?: number): string {
  if (!Number.isFinite(bytes) || bytes <= 0) return '0 B'
  const k = 1000
  const units = ['B', 'kB', 'MB', 'GB', 'TB', 'PB']
  const i = Math.max(0, Math.min(Math.floor(Math.log(bytes) / Math.log(k)), units.length - 1))
  const v = bytes / Math.pow(k, i)
  const places = decimals ?? (i >= 3 ? 2 : 1)
  return `${v.toFixed(i === 0 ? 0 : places)} ${units[i]}`
}

/** The live transfer counter — the same formatting, named for where it's used. */
export function formatBytesLive(bytes: number): string {
  return formatBytes(bytes)
}

// Whether to show speeds in megaBITS/sec (Mbps) vs megaBYTES/sec (MB/s). Set
// once from settings (setSpeedUnit) so every formatSpeed call stays consistent
// without threading the preference through every component.
let SPEED_IN_MEGABITS = false
export function setSpeedUnit(megabits: boolean): void {
  SPEED_IN_MEGABITS = megabits
}

// `bytesPerSec` is BYTES per second (despite the legacy name).
export function formatSpeed(bytesPerSec: number, megabits = SPEED_IN_MEGABITS): string {
  if (!Number.isFinite(bytesPerSec) || bytesPerSec <= 0) return '—'
  if (megabits) {
    const mbps = (bytesPerSec * 8) / 1_000_000
    return `${mbps.toFixed(mbps < 10 ? 1 : 0)} Mbps`
  }
  // Speeds stay at one decimal: a GB/s rate doesn't need hundredths.
  return `${formatBytes(bytesPerSec, 1)}/s`
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
  if (!Number.isFinite(ms) || Math.abs(ms) > 8.64e15) return '—'
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
