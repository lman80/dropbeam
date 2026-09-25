import type { TransferUpdate } from '../lib/api'
import { formatEta, formatSpeed } from '../lib/format'
import { useStore } from '../store'
import { IntegrityDetails } from './IntegrityDetails'
import { ConnInfo } from './ConnInspector'
import { ProgressBar } from './ui'

const UNITS = ['B', 'kB', 'MB', 'GB', 'TB']
/** "1.9 of 4.8 GB" — both figures in the total's unit, so the line doesn't jump. */
function ofBytes(done: number, total: number): string {
  const i = total > 0 ? Math.max(0, Math.min(Math.floor(Math.log(total) / Math.log(1000)), UNITS.length - 1)) : 0
  const f = (b: number) => (b / 1000 ** i).toFixed(i >= 2 ? 1 : 0)
  return `${f(Math.max(0, Math.min(done, total)))} of ${f(total)} ${UNITS[i]}`
}

/** The quiet status under a file in a chat thread: a thin bar and one line while
 *  it moves, "Couldn't send · Retry" when it fails, and nothing once it's done
 *  (a checksum mismatch is the only thing that speaks up after completion).
 *  Connection details sit behind the ⓘ, never inline. */
export function ChatTransferProgress({ t, onRetry }: { t: TransferUpdate; onRetry?: () => void }) {
  const megabits = useStore((s) => s.settings?.showMegabits ?? false)
  const send = t.direction === 'send'

  if (t.state === 'completed') {
    return <IntegrityDetails rows={t.integrity} total={t.bytesTotal} completed />
  }
  if (t.state === 'failed') {
    return (
      <div className="xfer-line failed" title={t.error ?? undefined}>
        <span>{send ? 'Couldn’t send' : 'Couldn’t receive'}</span>
        {onRetry && (
          <button type="button" className="btn btn-plain btn-sm xfer-retry" onClick={onRetry}>
            Retry
          </button>
        )}
      </div>
    )
  }
  if (t.state === 'canceled') return <div className="xfer-line">Canceled</div>

  const moving = t.state === 'transferring'
  const paused = t.state === 'paused'
  const label = moving
    ? `${ofBytes(t.bytesDone, t.bytesTotal)}${t.etaSeconds != null && t.etaSeconds > 0 ? ` · ${formatEta(t.etaSeconds)} left` : ''}`
    : paused ? `Paused · ${ofBytes(t.bytesDone, t.bytesTotal)}`
      : t.state === 'waitingForAccept' ? (send ? `Waiting for ${t.friendName ?? 'them'} to accept` : 'Waiting to accept')
        : t.state === 'connecting' ? 'Connecting…'
          : t.detail || (send ? 'Waiting to send…' : 'Waiting…')
  const speed = moving && t.speedBps > 0 ? formatSpeed(t.speedBps, megabits) : undefined

  return (
    <div className="xfer-status">
      {(moving || paused) && (
        <ProgressBar percent={t.percent} tone={paused ? 'paused' : undefined} label={send ? 'Sending' : 'Receiving'} />
      )}
      <div className="xfer-line">
        <span className="tnum truncate-1" title={speed}>{label}</span>
        <ConnInfo detail={t.connDetail} locality={t.locality} align="end" />
      </div>
    </div>
  )
}
