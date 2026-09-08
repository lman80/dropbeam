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
