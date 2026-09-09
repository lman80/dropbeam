import { CheckCircle2, AlertCircle } from 'lucide-react'
import type { FileIntegrity } from '../lib/api'
import { integrityLabel } from '../lib/integrity'

/** Native details makes hashes selectable and accessible on every card surface. */
export function IntegrityDetails({ rows = [], total, completed }: {
  rows?: FileIntegrity[]; total: number; completed: boolean
}) {
  if (!rows.length) return completed ? <div style={{ fontSize: 'calc(12px * var(--ui-font-scale, 1))', marginTop: 6, color: 'var(--text-muted)' }}>Saved, unverified</div> : null
  const label = integrityLabel(rows, total, completed)
  const failed = label === 'Verification failed — retry'
  const verified = label === 'Verified'
  const Icon = failed ? AlertCircle : CheckCircle2
  const tooltip = rows.map(r => `${r.name}\n${r.algorithm}\nLocal: ${r.digest}\nPeer: ${r.peerDigest}`).join('\n\n')
  return <details style={{ fontSize: 'calc(12px * var(--ui-font-scale, 1))', marginTop: 6, minWidth: 0 }}>
    <summary title={tooltip} style={{ cursor: 'pointer', color: failed ? 'var(--red)' : verified ? 'var(--green)' : 'var(--text-muted)' }}>
      {(failed || verified) && <Icon size={13} style={{ verticalAlign: 'middle', marginRight: 4 }} />}
      {label}
    </summary>
    {rows.map((r, index) => <div key={`${index}:${r.name}`} style={{ marginTop: 8, overflowWrap: 'anywhere', userSelect: 'text' }}>
      <b>{r.name}</b> · {r.size} bytes · {!r.verified ? 'Mismatch' : r.acknowledged ? 'Verified' : 'Saved, unverified'}
      <div>{r.algorithm}</div>
      <div>Local: <code>{r.digest}</code></div>
      <div>Peer: <code>{r.peerDigest}</code></div>
    </div>)}
  </details>
}
