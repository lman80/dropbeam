import { AlertCircle, ShieldCheck } from 'lucide-react'
import type { FileIntegrity } from '../lib/api'
import { integrityLabel } from '../lib/integrity'
import { InfoButton } from './ui'

/** Integrity is a promise the app keeps quietly. Primary UI only speaks up when a
 *  copy did NOT match; the checksums themselves live behind an info button. */
export function IntegrityDetails({ rows = [], total, completed }: {
  rows?: FileIntegrity[]; total: number; completed: boolean
}) {
  if (!rows.length) return null
  const label = integrityLabel(rows, total, completed)
  if (label !== 'Verification failed — retry') return null
  return (
    <div className="integrity-fail">
      <AlertCircle size={13} />
      <span>The copy didn’t match the original.</span>
      <IntegrityInfo rows={rows} />
    </div>
  )
}

/** ⓘ popover with the per-file checksums, for the rare person who wants them. */
export function IntegrityInfo({ rows = [] }: { rows?: FileIntegrity[] }) {
  if (!rows.length) return null
  const ok = rows.every((r) => r.verified)
  return (
    <InfoButton label="Checksums" icon={<ShieldCheck />} width={320} align="end">
      <h4>{ok ? 'Checked end to end' : 'Some files didn’t match'}</h4>
      <p>{ok ? 'Each file’s checksum matched on both devices.' : 'Send the files again to replace the bad copy.'}</p>
      {rows.map((r, index) => (
        <div key={`${index}:${r.name}`} className="integrity-row">
          <div className="integrity-name">{r.name}{!r.verified && <span className="integrity-bad"> · mismatch</span>}</div>
          <code>{r.algorithm} {r.digest.slice(0, 16)}…</code>
        </div>
      ))}
    </InfoButton>
  )
}
