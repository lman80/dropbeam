import type { ReactNode } from 'react'
import { Cloud, Loader2, Wifi, Zap } from 'lucide-react'
import type { Locality } from '../lib/api'

export function BeamLogo({ size = 20 }: { size?: number }) {
  const id = `bg-${size}`
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" fill="none" aria-hidden>
      <defs>
        <linearGradient id={id} x1="0" y1="0" x2="24" y2="24" gradientUnits="userSpaceOnUse">
          <stop offset="0" stopColor="var(--accent)" />
          <stop offset="1" stopColor="var(--accent-2)" />
        </linearGradient>
      </defs>
      <rect width="24" height="24" rx="7" fill={`url(#${id})`} />
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
    bg: 'var(--green-soft)',
    fg: 'var(--green)',
    tip: 'Direct peer-to-peer — your files go straight to the other device, end-to-end encrypted, no middleman. Fastest.',
  },
  local: {
    icon: Wifi,
    label: 'Local network',
    bg: 'var(--accent-soft)',
    fg: 'var(--accent)',
    tip: "Same network — sending over your local Wi-Fi/LAN. Very fast, and it never leaves your network.",
  },
  internet: {
    icon: Cloud,
    label: 'Relay',
    bg: 'var(--amber-soft)',
    fg: 'var(--amber)',
    tip: "Relayed — a direct link couldn't be made (strict network), so files hop through an encrypted relay. Slower, still private.",
  },
  unknown: {
    icon: Loader2,
    label: 'Connecting',
    bg: 'var(--surface-2)',
    fg: 'var(--text-faint)',
    tip: 'Finding the best route to the other device…',
  },
} as const

export function ChannelBadge({
  locality,
  size = 12,
  showConnecting = false,
}: {
  locality: Locality
  size?: number
  showConnecting?: boolean
}) {
  if (locality === 'unknown' && !showConnecting) return null
  const c = CHANNELS[locality] ?? CHANNELS.unknown
  const Icon = c.icon
  return (
    <span className="chip" title={c.tip} style={{ background: c.bg, color: c.fg }}>
      <Icon size={size} className={locality === 'unknown' ? 'animate-spin-slow' : undefined} />{' '}
      {c.label}
    </span>
  )
}

/** Back-compat alias — older call sites still import LocalityBadge. */
export function LocalityBadge({ locality }: { locality: Locality }) {
  return <ChannelBadge locality={locality} />
}

export function ProgressBar({ percent }: { percent: number }) {
  return (
    <div
      style={{
        height: 8,
        borderRadius: 999,
        background: 'var(--surface-2)',
        overflow: 'hidden',
        border: '1px solid var(--border)',
      }}
    >
      <div
        style={{
          height: '100%',
          width: `${Math.max(1.5, Math.min(100, percent))}%`,
          borderRadius: 999,
          background: 'linear-gradient(90deg, var(--accent), var(--accent-2))',
          transition: 'width 0.3s cubic-bezier(0.4, 0, 0.2, 1)',
        }}
      />
    </div>
  )
}

export function Spinner({ size = 16 }: { size?: number }) {
  return (
    <span
      className="animate-spin-slow"
      style={{
        display: 'inline-block',
        width: size,
        height: size,
        borderRadius: '50%',
        border: `2px solid color-mix(in srgb, var(--accent) 30%, transparent)`,
        borderTopColor: 'var(--accent)',
      }}
    />
  )
}

export function EmptyState({
  icon,
  title,
  hint,
}: {
  icon: ReactNode
  title: string
  hint?: string
}) {
  return (
    <div
      style={{
        display: 'flex',
        flexDirection: 'column',
        alignItems: 'center',
        justifyContent: 'center',
        textAlign: 'center',
        padding: '48px 24px',
        color: 'var(--text-muted)',
      }}
    >
      <div
        style={{
          width: 56,
          height: 56,
          borderRadius: 16,
          display: 'grid',
          placeItems: 'center',
          background: 'var(--surface-2)',
          border: '1px solid var(--border)',
          color: 'var(--text-faint)',
          marginBottom: 14,
        }}
      >
        {icon}
      </div>
      <div style={{ fontWeight: 650, color: 'var(--text)', fontSize: 15 }}>{title}</div>
      {hint && (
        <div style={{ fontSize: 13, marginTop: 5, maxWidth: 320, lineHeight: 1.5 }}>{hint}</div>
      )}
    </div>
  )
}

export function SectionTitle({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        fontSize: 12,
        fontWeight: 700,
        letterSpacing: '0.04em',
        textTransform: 'uppercase',
        color: 'var(--text-faint)',
        marginBottom: 10,
      }}
    >
      {children}
    </div>
  )
}
