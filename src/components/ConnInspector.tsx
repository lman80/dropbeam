import { Cloud, Loader2, Wifi, Zap } from 'lucide-react'
import type { ConnDetail } from '../lib/api'

/** The connection inspector: shows EXACTLY how two computers are connected right
 *  now — the real path (LAN / hole-punched direct / relay), its latency, and a live
 *  "upgrading to direct…" hint while a hole-punch is forming. Used on transfer cards
 *  and next to a friend. Pass `compact` for a tight inline pill. */
export function ConnInspector({
  detail,
  compact = false,
}: {
  detail?: ConnDetail | null
  compact?: boolean
}) {
  if (!detail) return null
  const { path, rttMs, upgrading, relay } = detail
  const meta =
    path === 'local'
      ? { icon: <Wifi size={13} />, label: 'Local network', color: 'var(--accent)', hint: 'Same Wi-Fi — fastest' }
      : path === 'direct'
        ? { icon: <Zap size={13} />, label: 'Direct', color: 'var(--green)', hint: 'Peer-to-peer, no relay' }
        : path === 'relay'
          ? {
              icon: <Cloud size={13} />,
              label: relay ? `Relay · ${relay}` : 'Relay',
              color: 'var(--amber)',
              hint: 'Via a relay server — slower',
            }
          : { icon: <Loader2 size={13} className="spin" />, label: 'Connecting…', color: 'var(--text-faint)', hint: '' }

  return (
    <span className={`conn-insp${compact ? ' compact' : ''}`} title={meta.hint}>
      <span className="conn-insp-ic" style={{ color: meta.color }}>
        {meta.icon}
      </span>
      <span className="conn-insp-label" style={{ color: meta.color }}>
        {meta.label}
      </span>
      {rttMs != null && <span className="conn-insp-rtt">· {rttMs} ms</span>}
      {upgrading && (
        <span className="conn-insp-up">
          <Loader2 size={11} className="spin" /> upgrading to direct…
        </span>
      )}
    </span>
  )
}
