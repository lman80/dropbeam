import type { ReactNode } from 'react'
import { Cloud, Loader2, Wifi, Zap } from 'lucide-react'
import type { Locality } from '../lib/api'

/** The app mark: a flat accent tile with the beam glyph (no gradient). */
export function BeamLogo({ size = 20 }: { size?: number }) {
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden>
      <rect width="24" height="24" rx="6.5" fill="var(--accent)" />
      <path d="M5.8 12.4 L17.5 5.6 L13.6 18.4 L11.2 13.1 Z" fill="white" />
    </svg>
  )
}

// The channel a transfer is flowing over — Direct P2P / Local network / Relay /
// Connecting. Plain-English tooltips so beta users understand what each means and
// roughly how fast to expect. One component, reused on every surface.
const CHANNELS = {
  direct: {
    icon: Zap,
    label: 'Direct',
    bg: 'transparent',
    fg: 'var(--text-muted)',
    tip: 'Direct peer-to-peer — your files go straight to the other device, end-to-end encrypted, no middleman. Fastest.',
  },
  local: {
    icon: Wifi,
    label: 'Local network',
    bg: 'transparent',
    fg: 'var(--text-muted)',
    tip: "Same network — sending over your local Wi-Fi/LAN. Very fast, and it never leaves your network.",
  },
  internet: {
    icon: Cloud,
    label: 'Relay',
    bg: 'transparent',
    fg: 'var(--text-muted)',
    tip: "Relayed — a direct link couldn't be made (strict network), so files hop through an encrypted relay. Slower, still private.",
  },
  unknown: {
    icon: Loader2,
    label: 'Connecting',
    bg: 'transparent',
    fg: 'var(--text-faint)',
    tip: 'Finding the best route to the other device…',
  },
} as const

export function ChannelBadge({
  locality,
  size = 12,
  showConnecting = false,
  iconOnly = false,
}: {
  locality: Locality
  size?: number
  showConnecting?: boolean
  /** Just the tinted icon (tight spaces like the floating HUD); label in the tooltip. */
  iconOnly?: boolean
}) {
  if (locality === 'unknown' && !showConnecting) return null
  const c = CHANNELS[locality] ?? CHANNELS.unknown
  const Icon = c.icon
  if (iconOnly) {
    return (
      <span className="chip chip-icon" title={`${c.label} — ${c.tip}`} aria-label={c.label} style={{ background: c.bg, color: c.fg }}>
        <Icon size={size} className={locality === 'unknown' ? 'animate-spin-slow' : undefined} />
      </span>
    )
  }
  return (
    <span className="channel-label" title={c.tip} style={{ color: c.fg }}>
      <Icon size={size} className={locality === 'unknown' ? 'animate-spin-slow' : undefined} />
      {c.label}
    </span>
  )
}

/** Back-compat alias — older call sites still import LocalityBadge. */
export function LocalityBadge({ locality }: { locality: Locality }) {
  return <ChannelBadge locality={locality} />
}

// Shared primitives now live in ./ui — re-exported so older imports keep working.
export { ProgressBar, Spinner, EmptyState } from './ui'

export function SectionTitle({ children }: { children: ReactNode }) {
  return <h2 className="section-title">{children}</h2>
}
