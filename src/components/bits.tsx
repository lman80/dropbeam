import type { ReactNode } from 'react'
import { Globe, Wifi } from 'lucide-react'
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

export function LocalityBadge({ locality }: { locality: Locality }) {
  if (locality === 'local')
    return (
      <span className="chip" style={{ background: 'var(--green-soft)', color: 'var(--green)' }}>
        <Wifi size={12} /> Local network
      </span>
    )
  if (locality === 'internet')
    return (
      <span className="chip" style={{ background: 'var(--accent-soft)', color: 'var(--accent)' }}>
        <Globe size={12} /> Internet
      </span>
    )
  return null
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
