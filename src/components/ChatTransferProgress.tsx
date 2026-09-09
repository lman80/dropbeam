import type { TransferUpdate } from '../lib/api'
import { chatTransferLabel } from '../lib/chatTransfer'
import { formatBytesLive, formatEta, formatSpeed } from '../lib/format'
import { useStore } from '../store'
import { IntegrityDetails } from './IntegrityDetails'
import { ConnInspector } from './ConnInspector'
import { LocalityBadge, ProgressBar } from './bits'

export function ChatTransferProgress({ t }: { t: TransferUpdate }) {
  const megabits = useStore((s) => s.settings?.showMegabits ?? false)
  const label = chatTransferLabel(t)
  return <div style={{ padding: '8px 12px', fontSize: 12, minWidth: 0 }}>
    <div style={{ display: 'flex', flexWrap: 'wrap', alignItems: 'center', gap: 8, marginBottom: 6 }}>
      <span>{label}</span>
      {t.connDetail ? <ConnInspector detail={t.connDetail} compact /> : <LocalityBadge locality={t.locality} />}
    </div>
    {t.state === 'transferring' && <>
      <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8, marginBottom: 6 }}>
        <b>{Math.round(t.percent)}%</b><span>{formatBytesLive(t.bytesDone)} / {formatBytesLive(t.bytesTotal)}</span>
      </div>
      <ProgressBar percent={t.percent} />
      <div style={{ display: 'flex', justifyContent: 'space-between', gap: 8, marginTop: 6 }}>
        <span>{formatSpeed(t.speedBps, megabits)}</span><span>{formatEta(t.etaSeconds)} left</span>
      </div>
    </>}
    <IntegrityDetails rows={t.integrity} total={t.bytesTotal} completed={t.state === 'completed'} />
    {t.detail && <div>{t.detail}</div>}
    {t.state === 'failed' && <div style={{ color: 'var(--red)', overflowWrap: 'anywhere' }}>{t.error ?? 'The transfer failed.'}</div>}
  </div>
}
