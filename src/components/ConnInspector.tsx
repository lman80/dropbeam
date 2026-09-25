import { Cloud, Info, Wifi, Zap } from 'lucide-react'
import type { ConnDetail, Locality } from '../lib/api'
import { pathKind, pathLabel, pathSentence, type PathKind } from '../lib/humanize'
import { InfoButton, Spinner } from './ui'

const ICONS: Record<PathKind, typeof Wifi | null> = { local: Wifi, direct: Zap, relay: Cloud, connecting: null }

/** The connection details people rarely need: which path, latency, relay region,
 *  whether it's switching to direct. Shown only on request, behind an ⓘ button,
 *  never as primary UI text. */
export function ConnInfo({
  detail,
  locality,
  label = 'Connection details',
  align = 'end',
}: {
  detail?: ConnDetail | null
  locality?: Locality | null
  label?: string
  align?: 'start' | 'end' | 'center'
}) {
  const kind = pathKind(detail, locality)
  if (!kind) return null
  const Icon = ICONS[kind]
  return (
    <InfoButton label={label} icon={<Info />} align={align} width={250} className="conn-info-btn">
      <h4 style={{ display: 'flex', alignItems: 'center', gap: 6 }}>
        {Icon ? <Icon size={14} /> : <Spinner size={12} />}
        {pathLabel(kind)}
      </h4>
      <p>{pathSentence(kind)}</p>
      {(detail?.rttMs != null || detail?.relay || detail?.upgrading) && (
        <dl className="kv">
          {detail?.rttMs != null && (<><dt>Latency</dt><dd>{detail.rttMs} ms</dd></>)}
          {detail?.relay && (<><dt>Relay</dt><dd>{detail.relay}</dd></>)}
          {detail?.upgrading && (<><dt>Status</dt><dd>Trying to connect directly…</dd></>)}
        </dl>
      )}
    </InfoButton>
  )
}

/** Back-compat: the old inline inspector pill is now just a quiet human label
 *  (no latency, no relay code) with the details behind ConnInfo. */
export function ConnInspector({ detail }: { detail?: ConnDetail | null; compact?: boolean }) {
  const kind = pathKind(detail)
  if (!kind || kind === 'connecting') return null
  return <span className="channel-label">{pathLabel(kind)}</span>
}
