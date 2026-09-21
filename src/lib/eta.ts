/** ETA from the last three seconds of reported progress (landed bytes on sends).
 * The first landed frame is a baseline: its delay includes setup and the first
 * MiB, and is not a useful measurement of the current transfer rate.
 */
export class LandedEta {
  private points: { at: number; bytes: number }[] = []

  update(at: number, bytes: number, total: number): number | null {
    if (![at, bytes, total].every(Number.isFinite) || bytes <= 0 || total <= 0) return null
    if (bytes >= total) return 0
    const last = this.points.at(-1)
    if (last && (bytes < last.bytes || at < last.at)) this.points = []
    if (last && at === last.at) return null
    this.points.push({ at, bytes })
    const cutoff = at - 3000
    while (this.points.length > 2 && this.points[1].at <= cutoff) this.points.shift()
    const first = this.points[0]
    const second = this.points[1]
    if (!second || at - first.at < 200) return null
    // Interpolate the window boundary so a long gap does not retain old speed.
    const start = Math.max(first.at, cutoff)
    const baseline = first.bytes + (second.bytes - first.bytes)
      * (start - first.at) / (second.at - first.at)
    const speed = (bytes - baseline) / ((at - start) / 1000)
    return speed > 0 ? (total - bytes) / speed : null
  }
}

export interface RateSample {
  /** Monotonic timestamp (performance.now()) of the progress frame. */
  at: number
  /** Bytes landed at that moment. */
  bytes: number
}

/** How far back the LIVE rate looks. */
export const LIVE_WINDOW_MS = 5000

/** Bytes/sec across a run of (timestamp, bytesDone) samples. Pure, so the whole
 * speed/ETA display is unit-testable: null = nothing measurable yet (fewer than
 * two samples, or no time between them); 0 = measured, but nothing moved. */
export function sampleRate(samples: RateSample[]): number | null {
  if (samples.length < 2) return null
  const first = samples[0]
  const last = samples[samples.length - 1]
  const secs = (last.at - first.at) / 1000
  if (!(secs > 0)) return null
  const bytes = last.bytes - first.bytes
  return bytes > 0 ? bytes / secs : 0
}

/** Drop samples older than `window`, keeping the one that straddles its start so
 * a slow trickle still measures the whole window instead of a sliver of it. */
export function trimSamples(samples: RateSample[], now: number, window = LIVE_WINDOW_MS): RateSample[] {
  const cutoff = now - window
  let drop = 0
  while (drop + 1 < samples.length && samples[drop + 1].at <= cutoff) drop++
  return drop ? samples.slice(drop) : samples
}

/** Seconds left at `bps`, or null when that can't be answered yet. */
export function etaAt(bytesDone: number, bytesTotal: number, bps: number | null | undefined): number | null {
  if (bps == null || !(bps > 0) || !(bytesTotal > 0)) return null
  if (bytesDone >= bytesTotal) return 0
  return (bytesTotal - bytesDone) / bps
}

/** The two rates a transfer card shows, fed one sample per progress frame:
 *  • LIVE — bytes over the last few seconds, what the link is doing right now.
 *  • AVERAGE — bytes over the whole run, which is what a time-left estimate for
 *    a long transfer should be based on (it doesn't swing with every hiccup).
 * A transfer that resumes after Paused/Failed gets a fresh tracker, so the
 * average always measures the run you're actually watching. */
export class TransferRate {
  private window: RateSample[] = []
  private anchor: RateSample | null = null
  private latest: RateSample | null = null
  private held: number | null = null

  /** Feed one progress frame. Out-of-order or rewound frames are ignored. */
  update(at: number, bytes: number): void {
    if (!Number.isFinite(at) || !Number.isFinite(bytes) || bytes < 0) return
    if (this.latest && (at <= this.latest.at || bytes < this.latest.bytes)) return
    const sample = { at, bytes }
    this.anchor ??= sample
    this.latest = sample
    this.window = trimSamples([...this.window, sample], at)
  }

  /** How many samples have been seen — under two, the card falls back to the
   * engine's own numbers rather than showing nothing. */
  get count(): number {
    return this.window.length
  }

  /** When this run started measuring (null before the first frame). */
  get startedAt(): number | null {
    return this.anchor?.at ?? null
  }

  live(): number | null {
    const rate = sampleRate(this.window)
    // Hold the last measurement across a frame that can't be measured, so the
    // number never blinks away between updates.
    if (rate == null) return this.held
    this.held = rate
    return rate
  }

  average(): number | null {
    return this.anchor && this.latest ? sampleRate([this.anchor, this.latest]) : null
  }
}
