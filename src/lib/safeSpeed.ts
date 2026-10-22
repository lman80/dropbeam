import type { SafeSpeedProgress, SafeSpeedReport } from './api.ts'

/** The speeds the test tries, in Mbps — the same list as safe_speed.rs STEPS. */
export const STEPS_MBPS = [25, 50, 75, 100, 150, 200, 300, 400, 600, 800, 1000, 1500, 2500]

/** What "Find my safe speed" is doing right now, in plain words (#20). */
export function safeSpeedStep(p: SafeSpeedProgress | null): string {
  if (!p || p.phase === 'router') return 'Finding your router…'
  if (p.phase === 'device') return 'Finding a device to send test data to…'
  const rate = p.rateMbps ?? 0
  const router = p.health === 'trouble' ? 'your router is struggling' : p.health === 'strained' ? 'your router is slowing down' : 'your router is fine'
  return `Trying ${rate} Mbps${p.target ? ` to ${p.target}` : ''} — ${router}`
}

/** The result as a headline + one line of detail. */
export function safeSpeedResult(r: SafeSpeedReport): { title: string; detail: string; apply: number } {
  const where = r.local ? 'on your home network' : 'over the internet'
  if (r.kind === 'limit') {
    return {
      title: `We recommend ${r.safeMbps} Mbps`,
      detail: `Your router started to struggle at about ${r.troubleMbps} Mbps ${where}. ${r.safeMbps} Mbps leaves it room to breathe.`,
      apply: r.safeMbps,
    }
  }
  return {
    title: 'No limit needed',
    detail: `Your router kept up at full speed (${r.topMbps} Mbps ${where}).`,
    apply: 0,
  }
}

/** The settings to save when the person applies the result: the limit, and on
 *  the local network too when that's where the router struggled. */
export function applySafeSpeed(r: SafeSpeedReport): { uploadLimitMbps: number; limitLocal?: boolean } {
  if (r.kind === 'noLimit') return { uploadLimitMbps: 0 }
  return r.local ? { uploadLimitMbps: r.safeMbps, limitLocal: true } : { uploadLimitMbps: r.safeMbps }
}
