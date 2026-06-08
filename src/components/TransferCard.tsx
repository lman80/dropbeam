import { useState } from 'react'
import { motion } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import {
  AlertCircle,
  ArrowDownToLine,
  Check,
  CheckCircle2,
  Copy,
  FolderOpen,
  Send,
  X,
} from 'lucide-react'
import { api, isActive, type TransferUpdate } from '../lib/api'
import { formatBytes, formatEta, formatSpeed } from '../lib/format'
import { LocalityBadge, ProgressBar, Spinner } from './bits'
import { useStore } from '../store'

function title(t: TransferUpdate): string {
  if (t.fileNames.length === 1) return t.fileNames[0]
  if (t.fileNames.length > 1) return `${t.fileNames[0]} + ${t.fileNames.length - 1} more`
  return t.direction === 'receive' ? 'Incoming files' : 'Files'
}

export function TransferCard({ t }: { t: TransferUpdate }) {
  const removeTransfer = useStore((s) => s.removeTransfer)
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
      style={{ padding: 18, overflow: 'hidden' }}
    >
      {/* header */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
        <div
          style={{
            width: 38,
            height: 38,
            borderRadius: 11,
            display: 'grid',
            placeItems: 'center',
            flexShrink: 0,
            color: stateColor(t),
            background: `color-mix(in srgb, ${stateColor(t)} 14%, transparent)`,
          }}
        >
          {t.state === 'completed' ? (
            <CheckCircle2 size={20} />
          ) : t.state === 'failed' ? (
            <AlertCircle size={20} />
          ) : (
            <DirIcon size={19} />
          )}
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            className="selectable"
            style={{
              fontWeight: 650,
              fontSize: 14.5,
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
            }}
          >
            {title(t)}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 3 }}>
            <span style={{ fontSize: 12.5, color: 'var(--text-muted)' }}>{statusLabel(t)}</span>
            <LocalityBadge locality={t.locality} />
          </div>
        </div>
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

      {/* manual-accept offer from a friend */}
      {isOffer && (
        <div style={{ marginTop: 14 }}>
          <div style={{ fontSize: 13, color: 'var(--text-muted)', marginBottom: 12, lineHeight: 1.5 }}>
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
            gap: 18,
            marginTop: 16,
            alignItems: 'center',
            flexWrap: 'wrap',
          }}
        >
          <div style={{ flex: 1, minWidth: 220 }}>
            <div style={{ fontSize: 12.5, color: 'var(--text-muted)', marginBottom: 7 }}>
              {(t.code?.length ?? 0) > 40
                ? 'On the other device → Receive: scan the QR, or paste this Direct ticket:'
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
        <div style={{ marginTop: 16 }}>
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              alignItems: 'baseline',
              marginBottom: 8,
            }}
          >
            <span style={{ fontSize: 22, fontWeight: 750 }} className="gradient-text">
              {Math.round(t.percent)}%
            </span>
            <span style={{ fontSize: 12.5, color: 'var(--text-muted)' }}>
              {formatBytes(t.bytesDone)}
              {t.bytesTotal > 0 ? ` / ${formatBytes(t.bytesTotal)}` : ''}
            </span>
          </div>
          <ProgressBar percent={t.percent} />
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              marginTop: 9,
              fontSize: 12.5,
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
            marginTop: 14,
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
              marginTop: 14,
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
            marginTop: 14,
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
            <button className="btn btn-ghost" onClick={() => api.openPath(t.outDir!)}>
              <FolderOpen size={15} /> Open folder
            </button>
          )}
        </div>
      )}

      {/* failed */}
      {t.state === 'failed' && (
        <div
          style={{
            marginTop: 12,
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
  }
}
