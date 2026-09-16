import { memo, useState } from 'react'
import { motion } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import {
  AlertCircle,
  ArrowDownToLine,
  Check,
  CheckCircle2,
  Copy,
  FolderOpen,
  Loader2,
  Pause,
  PauseCircle,
  Play,
  RotateCw,
  Send,
  X,
} from 'lucide-react'
import { api, isActive, type TransferUpdate } from '../lib/api'
import { formatBytes, formatBytesLive, formatEta, formatSpeed as formatSpeedValue } from '../lib/format'
import { LocalityBadge, ProgressBar, Spinner } from './bits'
import { IntegrityDetails } from './IntegrityDetails'
import { ConnInspector } from './ConnInspector'
import { useStore } from '../store'

function title(t: TransferUpdate): string {
  if (t.fileNames.length === 1) return t.fileNames[0]
  if (t.fileNames.length > 1) return `${t.fileNames[0]} + ${t.fileNames.length - 1} more`
  return t.direction === 'receive' ? 'Incoming files' : 'Files'
}

// Memoized: upsertTransfer only mints a fresh object for the id that changed, so
// a referential-equality memo stops every OTHER card re-rendering (with its
// framer-motion layout) on every progress tick.
export const TransferCard = memo(TransferCardImpl)

function TransferCardImpl({ t }: { t: TransferUpdate }) {
  const showMegabits = useStore((s) => s.settings?.showMegabits ?? false)
  const formatSpeed = (bps: number) => formatSpeedValue(bps, showMegabits)
  const removeTransfer = useStore((s) => s.removeTransfer)
  const retryTransfer = useStore((s) => s.retryTransfer)
  const respondToOffer = useStore((s) => s.respondToOffer)
  const toast = useStore((s) => s.toast)
  const summary = useStore((s) => s.transferSummaries[t.id])
  const [copied, setCopied] = useState(false)

  const active = isActive(t.state)
  const isOffer = t.state === 'waitingForAccept'
  // A friend send has no code to show — it rides a pre-shared channel.
  const isSendWaiting =
    t.direction === 'send' &&
    (t.state === 'waitingForPeer' || t.state === 'starting') &&
    !!t.code &&
    !t.friendName
  // Pause is for SENDS we're driving: stopping keeps every byte already delivered,
  // so you can leave the network now and finish later from the same card.
  const canPause =
    t.direction === 'send' &&
    (t.state === 'starting' ||
      t.state === 'waitingForPeer' ||
      t.state === 'connecting' ||
      t.state === 'transferring')
  const isFriendPending =
    t.direction === 'send' &&
    !!t.friendName &&
    (t.state === 'starting' || t.state === 'waitingForPeer' || t.state === 'connecting')

  const copyCode = async () => {
    if (!t.code) return
    try {
      await navigator.clipboard.writeText(t.code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    } catch {
      toast('error', 'Could not copy to clipboard')
    }
  }

  const DirIcon = t.direction === 'send' ? Send : ArrowDownToLine

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 12, scale: 0.99 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.97, transition: { duration: 0.15 } }}
      transition={{ type: 'spring', stiffness: 320, damping: 28 }}
      className="card"
      style={{ padding: 13, overflow: 'hidden' }}
    >
      {/* header */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
        <div
          style={{
            width: 30,
            height: 30,
            borderRadius: 9,
            display: 'grid',
            placeItems: 'center',
            flexShrink: 0,
            color: stateColor(t),
            background: `color-mix(in srgb, ${stateColor(t)} 14%, transparent)`,
          }}
        >
          {t.state === 'completed' ? (
            <CheckCircle2 size={17} />
          ) : t.state === 'failed' ? (
            <AlertCircle size={17} />
          ) : t.state === 'paused' ? (
            <PauseCircle size={17} />
          ) : (
            <DirIcon size={16} />
          )}
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            className="selectable"
            style={{
              fontWeight: 650,
              fontSize: 13.5,
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
            }}
          >
            {title(t)}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 3 }}>
            <span style={{ fontSize: 12.5, color: 'var(--text-muted)' }}>{statusLabel(t)}{t.locationSkipped ? ` · ${t.locationSkipped} ${t.locationSkipped === 1 ? 'file' : 'files'} already there` : ''}{t.locationConflicts ? ` · ${t.locationConflicts} ${t.locationConflicts === 1 ? 'file' : 'files'} already existed with different content — saved next to them as ‘… (2)’` : ''}</span>
            {t.connDetail ? (
              <ConnInspector detail={t.connDetail} compact />
            ) : (
              <LocalityBadge locality={t.locality} />
            )}
          </div>
        </div>
        {canPause && (
          <button
            className="icon-btn"
            title="Pause — keeps what's already been sent"
            onClick={() => void api.pauseTransfer(t.id)}
          >
            <Pause size={15} />
          </button>
        )}
        <button
          className="icon-btn"
          title={isOffer ? 'Decline' : active ? 'Cancel' : 'Dismiss'}
          onClick={() =>
            isOffer
              ? respondToOffer(t.id, false)
              : active
                ? api.cancelTransfer(t.id)
                : removeTransfer(t.id)
          }
        >
          <X size={16} />
        </button>
      </div>

      {/* Parked: "Wait for a direct connection" is holding this off the relay.
          The escape-hatch button only makes sense while we're still parked
          (waitingForPeer); once we've fallen through to the relay it's just an
          informational line. */}
      {t.detail && active && (
        <div className="conn-park">
          <Loader2 size={14} className="spin" />
          <span style={{ flex: 1, minWidth: 0 }}>{t.detail}</span>
          {t.state === 'waitingForPeer' && (
            <button className="btn btn-ghost" onClick={() => void api.forceRelay(t.id)}>
              Send over relay anyway
            </button>
          )}
        </div>
      )}

      {/* manual-accept offer from a friend */}
      {isOffer && (
        <div style={{ marginTop: 10 }}>
          <div style={{ fontSize: 13, color: 'var(--text-muted)', marginBottom: 10, lineHeight: 1.5 }}>
            <b style={{ color: 'var(--text)' }}>{t.friendName ?? 'Someone'}</b> wants to send you{' '}
            <b style={{ color: 'var(--text)' }}>
              {t.fileNames.length ? t.fileNames[0] : 'files'}
            </b>
            {t.bytesTotal > 0 ? ` · ${formatBytes(t.bytesTotal)}` : ''}
          </div>
          <div style={{ display: 'flex', gap: 10 }}>
            <button
              className="btn btn-primary"
              style={{ flex: 1 }}
              onClick={() => respondToOffer(t.id, true)}
            >
              <Check size={16} /> Accept
            </button>
            <button
              className="btn btn-ghost"
              style={{ flex: 1 }}
              onClick={() => respondToOffer(t.id, false)}
            >
              <X size={16} /> Decline
            </button>
          </div>
        </div>
      )}

      {/* send waiting: code + QR */}
      {isSendWaiting && (
        <div
          style={{
            display: 'flex',
            gap: 16,
            marginTop: 12,
            alignItems: 'center',
            flexWrap: 'wrap',
          }}
        >
          <div style={{ flex: 1, minWidth: 220 }}>
            <div style={{ fontSize: 12.5, color: 'var(--text-muted)', marginBottom: 7 }}>
              {(t.code?.length ?? 0) > 40
                ? 'On their DropBeam → Send & Receive → “Have a code?”: scan the QR or paste this code:'
                : 'On the other device, open DropBeam → Receive and enter:'}
            </div>
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 8,
                background: 'var(--surface-2)',
                border: '1px solid var(--border)',
                borderRadius: 13,
                padding: '10px 12px',
              }}
            >
              <code
                className="selectable"
                style={{
                  flex: 1,
                  fontFamily: 'var(--font-mono)',
                  fontSize: (t.code?.length ?? 0) > 40 ? 10.5 : 18,
                  lineHeight: (t.code?.length ?? 0) > 40 ? 1.45 : undefined,
                  maxHeight: (t.code?.length ?? 0) > 40 ? 58 : undefined,
                  overflowY: (t.code?.length ?? 0) > 40 ? 'auto' : undefined,
                  fontWeight: 600,
                  letterSpacing: '0.02em',
                  color: 'var(--text)',
                  wordBreak: 'break-all',
                }}
              >
                {t.code}
              </code>
              <button className={`btn ${copied ? 'btn-ghost' : 'btn-primary'}`} onClick={copyCode}>
                {copied ? <Check size={15} /> : <Copy size={15} />}
                {copied ? 'Copied' : 'Copy'}
              </button>
            </div>
            <div
              style={{
                display: 'flex',
                alignItems: 'center',
                gap: 8,
                marginTop: 12,
                fontSize: 12.5,
                color: 'var(--text-muted)',
              }}
            >
              <Spinner size={14} />
              Waiting for the other device to connect…
            </div>
          </div>
          <div
            style={{
              background: '#ffffff',
              padding: 12,
              borderRadius: 14,
              border: '1px solid var(--border)',
              flexShrink: 0,
            }}
          >
            <QRCodeSVG value={t.code!} size={116} level="M" fgColor="#15161d" bgColor="#ffffff" />
          </div>
        </div>
      )}

      {/* transferring: progress */}
      {t.state === 'transferring' && (
        <div style={{ marginTop: 10 }}>
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              alignItems: 'baseline',
              marginBottom: 6,
            }}
          >
            <span style={{ fontSize: 16, fontWeight: 750 }} className="gradient-text">
              {Math.round(t.percent)}%
            </span>
            <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>
              {formatBytesLive(t.bytesDone)}
              {t.bytesTotal > 0 ? ` / ${formatBytesLive(t.bytesTotal)}` : ''}
            </span>
          </div>
          <ProgressBar percent={t.percent} />
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              marginTop: 6,
              fontSize: 12,
              color: 'var(--text-muted)',
            }}
          >
            <span>{formatSpeed(t.speedBps)}</span>
            <span>{formatEta(t.etaSeconds)} left</span>
          </div>
        </div>
      )}

      {/* friend send: no code, just a calm "beaming to {name}" */}
      {isFriendPending && (
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 9,
            marginTop: 10,
            fontSize: 13,
            color: 'var(--text-muted)',
          }}
        >
          <Spinner size={15} />
          Beaming to {t.friendName}…
        </div>
      )}

      {/* connecting (receive or post-handshake) */}
      {!isFriendPending &&
        (t.state === 'connecting' || (t.state === 'starting' && !isSendWaiting)) && (
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 9,
              marginTop: 10,
              fontSize: 13,
              color: 'var(--text-muted)',
            }}
          >
            <Spinner size={15} />
            {t.direction === 'receive' ? 'Connecting to sender…' : 'Connecting…'}
          </div>
        )}

      {/* completed */}
      {t.state === 'completed' && (
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            justifyContent: 'space-between',
            marginTop: 10,
            gap: 12,
          }}
        >
          <div style={{ fontSize: 13, color: 'var(--text-muted)' }}>
            <div>
              {t.direction === 'receive' ? 'Saved' : 'Delivered'}
              {t.bytesTotal > 0 ? ` · ${formatBytes(t.bytesTotal)}` : ''}
            </div>
            {summary && (
              <div style={{ fontSize: 12, color: 'var(--text-faint)', marginTop: 2 }}>
                {formatEta(summary.durationMs / 1000)} · {formatSpeed(summary.avgBps)} avg
              </div>
            )}
          </div>
          {t.direction === 'receive' && t.outDir && (
            <button
              className="btn btn-ghost"
              onClick={() => {
                // A single file → reveal it SELECTED in its folder ("Show in
                // folder"); multiple → just open the folder. Match the folder's own
                // path separator so it works on Windows + macOS.
                const sep = t.outDir!.includes('\\') ? '\\' : '/'
                if (t.fileNames.length === 1) {
                  api.revealPath(`${t.outDir}${sep}${t.fileNames[0]}`).catch(() => {})
                } else {
                  api.openPath(t.outDir!).catch(() => {})
                }
              }}
            >
              <FolderOpen size={15} /> {t.fileNames.length === 1 ? 'Show in folder' : 'Open folder'}
            </button>
          )}
        </div>
      )}

      <IntegrityDetails rows={t.integrity} total={t.bytesTotal} completed={t.state === 'completed'} />

      {/* Verify copy: a full SHA-256 comparison of every file in this send against
          the copy that actually landed on the peer. Only a finished SEND has both
          sides to compare, and the peer reads at its own pace (a NAS manages
          ~10 MB/s), so a big folder shows live progress and can be canceled. */}
      {t.direction === 'send' && t.state === 'completed' && (
        <div style={{ marginTop: 10 }}>
          {t.verify?.state === 'running' ? (
            <>
              <div
                style={{
                  display: 'flex',
                  alignItems: 'baseline',
                  justifyContent: 'space-between',
                  gap: 8,
                  marginBottom: 6,
                }}
              >
                <span style={{ fontSize: 12.5, color: 'var(--text-muted)' }}>
                  Verifying… {t.verify.checked.toLocaleString()} /{' '}
                  {t.verify.total.toLocaleString()} files
                  {t.verify.bytesTotal > 0
                    ? ` · ${formatBytes(t.verify.bytesHashed)} / ${formatBytes(t.verify.bytesTotal)}`
                    : ''}
                </span>
                <button
                  className="btn btn-ghost"
                  style={{ padding: '2px 8px', fontSize: 12.5, flexShrink: 0 }}
                  onClick={() => void api.cancelVerify(t.id)}
                >
                  Cancel
                </button>
              </div>
              <ProgressBar
                percent={
                  t.verify.bytesTotal > 0
                    ? (t.verify.bytesHashed / t.verify.bytesTotal) * 100
                    : 0
                }
              />
            </>
          ) : t.verify?.state === 'done' &&
            t.verify.mismatched.length + t.verify.missing.length === 0 ? (
            <div
              style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 13, color: 'var(--green)' }}
            >
              <CheckCircle2 size={15} /> All {t.verify.total.toLocaleString()} files identical
            </div>
          ) : t.verify?.state === 'done' ? (
            <details>
              <summary style={{ cursor: 'pointer', fontSize: 13, color: 'var(--red)' }}>
                {(t.verify.mismatched.length + t.verify.missing.length).toLocaleString()} of{' '}
                {t.verify.total.toLocaleString()} files don’t match
              </summary>
              <div
                className="selectable"
                style={{
                  marginTop: 6,
                  maxHeight: 150,
                  overflowY: 'auto',
                  fontSize: 12,
                  color: 'var(--text-muted)',
                  lineHeight: 1.5,
                }}
              >
                {t.verify.mismatched.map((name) => (
                  <div key={`different:${name}`}>Different: {name}</div>
                ))}
                {t.verify.missing.map((name) => (
                  <div key={`missing:${name}`}>Missing: {name}</div>
                ))}
              </div>
            </details>
          ) : (
            <>
              {t.verify?.state === 'failed' && (
                <div style={{ fontSize: 12.5, color: 'var(--red)', marginBottom: 8, lineHeight: 1.45 }}>
                  {t.verify.error ?? 'Could not verify the copy.'}
                </div>
              )}
              {t.verify?.state === 'canceled' && (
                <div style={{ fontSize: 12.5, color: 'var(--text-muted)', marginBottom: 8 }}>
                  Verification canceled.
                </div>
              )}
              <button
                className="btn btn-ghost"
                style={{ width: '100%' }}
                onClick={() => {
                  void api.verifyTransfer(t.id).catch((e) => toast('error', String(e)))
                }}
              >
                <Check size={15} /> Verify copy
              </button>
            </>
          )}
        </div>
      )}

      {/* failed */}
      {t.state === 'failed' && (
        <div style={{ marginTop: 12 }}>
          <div
            style={{
              fontSize: 13,
              color: 'var(--red)',
              background: 'var(--red-soft)',
              borderRadius: 11,
              padding: '10px 12px',
              lineHeight: 1.45,
            }}
          >
            {t.error ?? 'The transfer failed.'}
          </div>
          {/* One-tap re-send, only on a failed SEND (a failed receive has no original
              paths/recipient to replay — retryTransfer is a no-op there). */}
          {t.direction === 'send' && (
            <button
              className="btn btn-ghost"
              style={{ marginTop: 10, width: '100%' }}
              onClick={() => void retryTransfer(t.id)}
            >
              <RotateCw size={15} /> Retry
            </button>
          )}
        </div>
      )}

      {/* paused: how far it got + one-tap Resume (replays the same send; everything
          already delivered is skipped). Cancel/Dismiss stays in the header. */}
      {t.state === 'paused' && (
        <div style={{ marginTop: 12 }}>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              fontSize: 13,
              color: 'var(--text-muted)',
              background: 'var(--surface-2)',
              borderRadius: 11,
              padding: '10px 12px',
              lineHeight: 1.45,
            }}
          >
            <PauseCircle size={15} style={{ flexShrink: 0 }} />
            <span>
              {t.detail ?? 'Paused — resume any time'}
              {t.bytesTotal > 0
                ? ` · ${formatBytes(t.bytesDone)} of ${formatBytes(t.bytesTotal)} done`
                : ''}
            </span>
          </div>
          {t.bytesTotal > 0 && (
            <div style={{ marginTop: 8 }}>
              <ProgressBar percent={t.percent} />
            </div>
          )}
          {t.direction === 'send' && (
            <button
              className="btn btn-ghost"
              style={{ marginTop: 10, width: '100%' }}
              onClick={() => void retryTransfer(t.id)}
            >
              <Play size={15} /> Resume
            </button>
          )}
        </div>
      )}

      {t.state === 'canceled' && (
        <div style={{ marginTop: 12, fontSize: 13, color: 'var(--text-muted)' }}>
          Transfer canceled.
        </div>
      )}
    </motion.div>
  )
}

function stateColor(t: TransferUpdate): string {
  if (t.state === 'completed') return 'var(--green)'
  if (t.state === 'failed') return 'var(--red)'
  if (t.state === 'canceled') return 'var(--text-faint)'
  if (t.state === 'paused') return 'var(--text-muted)'
  return 'var(--accent)'
}

function statusLabel(t: TransferUpdate): string {
  const fn = t.friendName
  const send = t.direction === 'send'
  switch (t.state) {
    case 'starting':
      return send && fn ? `Beaming to ${fn}…` : 'Starting…'
    case 'waitingForPeer':
      return send && fn ? `Beaming to ${fn}…` : 'Ready to send'
    case 'connecting':
      return fn ? (send ? `Beaming to ${fn}…` : `Receiving from ${fn}…`) : 'Connecting…'
    case 'waitingForAccept':
      return fn ? `${fn} wants to send files` : 'Incoming files'
    case 'transferring':
      return send ? (fn ? `Sending to ${fn}` : 'Sending') : fn ? `Receiving from ${fn}` : 'Receiving'
    case 'completed':
      return send ? (fn ? `Sent to ${fn}` : 'Sent') : fn ? `Received from ${fn}` : 'Received'
    case 'failed':
      return 'Failed'
    case 'canceled':
      return !send && fn ? 'Declined' : 'Canceled'
    case 'paused':
      return fn ? `Paused — sending to ${fn}` : 'Paused'
  }
}
