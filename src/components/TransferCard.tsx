import { FileIcon } from './FileIcon'
import { integrityLabel } from '../lib/integrity'
import { ShareFilesButton } from './ShareFilesButton'
import { MOBILE_UI } from '../lib/platform'
import { memo, useState } from 'react'
import { motion } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import { ShareCode } from './CodeQr'
import {
  AlertCircle,
  ArrowDownToLine,
  Check,
  CheckCircle2,
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

function TransferCardImpl({ t, onRetry, onShow, showAction = true }: { t: TransferUpdate; onRetry?: () => void; onShow?: () => void; showAction?: boolean }) {
  const showMegabits = useStore((s) => s.settings?.showMegabits ?? false)
  const formatSpeed = (bps: number) => formatSpeedValue(bps, showMegabits)
  const removeTransfer = useStore((s) => s.removeTransfer)
  const retryTransfer = useStore((s) => s.retryTransfer)
  const respondToOffer = useStore((s) => s.respondToOffer)
  const toast = useStore((s) => s.toast)
  const summary = useStore((s) => s.transferSummaries[t.id])
  const rates = useStore((s) => s.transferRates[t.id])
  const speedMode = useStore((s) => s.speedMode)
  const etaMode = useStore((s) => s.etaMode)
  const toggleSpeedMode = useStore((s) => s.toggleSpeedMode)
  const toggleEtaMode = useStore((s) => s.toggleEtaMode)
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

  // By default the SPEED is live (what the link is doing right now) and the TIME
  // LEFT is based on the whole-transfer average (which doesn't swing with every
  // hiccup). Clicking either swaps its basis; both choices are global and stick.
  const engineBps = t.speedBps > 0 ? t.speedBps : null
  const shownBps =
    (speedMode === 'live' ? rates?.liveBps ?? rates?.avgBps : rates?.avgBps ?? rates?.liveBps) ??
    engineBps
  const shownEta =
    (etaMode === 'avg' ? rates?.avgEta ?? rates?.liveEta : rates?.liveEta ?? rates?.avgEta) ??
    t.etaSeconds
  // Neither figure blinks between frames: the live rate holds its last reading
  // across a frame it can't measure, and a stall is named rather than shown as a
  // dash. Only the first few seconds say "calculating…".
  const settling = (rates?.ageMs ?? 0) < 3000
  const speedText =
    shownBps == null
      ? settling
        ? 'calculating…'
        : '—'
      : shownBps > 0
        ? `${formatSpeed(shownBps)} ${speedMode}`
        : 'stalled'
  const etaText =
    shownEta == null
      ? settling
        ? 'calculating…'
        : '— left'
      : `${formatEta(shownEta)} left · ${etaMode}`

  if (MOBILE_UI) {
    const route = t.connDetail?.path === 'relay' || t.locality === 'internet' ? 'Relay' : t.connDetail?.path === 'direct' || t.connDetail?.path === 'local' || t.locality === 'direct' || t.locality === 'local' ? 'Direct' : ''
    const verified = integrityLabel(t.integrity ?? [], t.bytesTotal, t.state === 'completed')
    const show = t.state === 'completed' && (!!t.outDir || !!onShow)
    const retry = t.state === 'failed' && t.direction === 'send'
    const action = show ? 'Show' : retry ? 'Retry' : active ? 'Cancel' : 'Dismiss'
    return <article className="mobile-transfer">
      <div className="mobile-transfer-heading"><span className="mobile-tinted-icon"><FileIcon name={t.fileNames[0] ?? ''} size={22} /></span>
        <div className="mobile-grow"><h3 className="ios-headline mobile-ellipsis">{title(t)}</h3><p className="ios-footnote mobile-ellipsis">{formatBytes(t.bytesTotal)} · {t.friendName ?? t.peer ?? 'Peer'}</p></div>
        {showAction && <button className="ios-icon" aria-label={action} onClick={() => {
          if (show && onShow) onShow()
          else if (show) void api.shareFiles(t.fileNames.map(n => `${t.outDir}/${n}`)).catch(e => toast('error', String(e)))
          else if (retry) { if (onRetry) onRetry(); else void retryTransfer(t.id) }
          else if (isOffer) void respondToOffer(t.id, false)
          else if (active) void api.cancelTransfer(t.id)
          else removeTransfer(t.id)
        }}>{show ? <FolderOpen size={21} /> : retry ? <RotateCw size={21} /> : <X size={21} />}</button>}
      </div>
      {t.state === 'completed' ? <p className="ios-footnote">{t.direction === 'send' ? 'Delivered' : 'Saved'}{route && ` · ${route}`} · {verified}</p> : <>
        <div className="mobile-progress" role="progressbar" aria-label="Transfer progress" aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(t.percent)}><span style={{ width: `${Math.max(0, Math.min(100, t.percent))}%` }} /></div>
        <div className="mobile-transfer-stats ios-footnote"><button className="xfer-meter" onClick={toggleSpeedMode}>{formatBytesLive(t.bytesDone)} of {formatBytesLive(t.bytesTotal)} · {speedText}</button><button className="xfer-meter" onClick={toggleEtaMode}>{etaText}</button></div>
        <div className="mobile-transfer-badges">{route && <span className="ios-caption">{route}</span>}{verified === 'Verified' && <span className="ios-caption">Verified</span>}</div>
        {t.state !== 'transferring' && <p className="ios-footnote">{t.error ?? t.detail ?? statusLabel(t)}</p>}
      </>}
      {isOffer && <button className="ios-button ios-primary" onClick={() => respondToOffer(t.id, true)}>Accept files</button>}
      {isSendWaiting && <div className="mobile-stack"><code className="mobile-transfer-code">{t.code}</code><button className="ios-button" onClick={copyCode}>{copied ? 'Copied' : 'Copy code'}</button><div className="mobile-qr"><QRCodeSVG value={t.code!} size={116} /></div></div>}
      {t.detail && t.state === 'waitingForPeer' && <button className="ios-button" onClick={() => void api.forceRelay(t.id)}>Send over relay anyway</button>}
    </article>
  }

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 12, scale: 0.99 }}
      animate={{ opacity: 1, y: 0, scale: 1 }}
      exit={{ opacity: 0, scale: 0.97, transition: { duration: 0.15 } }}
      transition={{ type: 'spring', stiffness: 320, damping: 28 }}
      className="card xfer-card"
      style={{ padding: 14, overflow: 'hidden' }}
    >
      {/* header */}
      <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
        <div
          style={{
            width: 30,
            height: 30,
            borderRadius: 'var(--radius-sm)',
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
              fontSize: 'var(--font-base)',
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
            }}
            title={t.fileNames.join('\n') || undefined}
          >
            {title(t)}
          </div>
          <div style={{ display: 'flex', alignItems: 'center', gap: '2px 8px', marginTop: 3, flexWrap: 'wrap', minWidth: 0 }}>
            <span style={{ fontSize: 'var(--font-sm)', color: 'var(--text-muted)', minWidth: 0, overflowWrap: 'anywhere' }}>{statusLabel(t)}{t.locationSkipped ? ` · ${t.locationSkipped} ${t.locationSkipped === 1 ? 'file' : 'files'} already there` : ''}{t.locationConflicts ? ` · ${t.locationConflicts} ${t.locationConflicts === 1 ? 'file' : 'files'} already existed with different content — saved next to them as ‘… (2)’` : ''}{t.locationReplaced ? ` · ${t.locationReplaced} ${t.locationReplaced === 1 ? 'file' : 'files'} updated — the older version is in the folder’s Trash` : ''}</span>
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
            aria-label="Pause"
            onClick={() => void api.pauseTransfer(t.id)}
          >
            <Pause size={15} />
          </button>
        )}
        <button
          className="icon-btn"
          title={isOffer ? 'Decline' : active ? 'Cancel' : 'Dismiss'}
          aria-label={isOffer ? 'Decline' : active ? 'Cancel' : 'Dismiss'}
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
            <button className="btn btn-ghost btn-sm" onClick={() => void api.forceRelay(t.id)}>
              Send over relay anyway
            </button>
          )}
        </div>
      )}

      {/* manual-accept offer from a friend */}
      {isOffer && (
        <div style={{ marginTop: 10 }}>
          <div style={{ fontSize: 'var(--font-sm)', color: 'var(--text-muted)', marginBottom: 10, lineHeight: 1.5 }}>
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

      {/* send waiting: QR + code (anyone can scan it with DropBeam, or paste it) */}
      {isSendWaiting && (
        <div style={{ marginTop: 14 }}>
          <ShareCode
            code={t.code!}
            size={184}
            instructions={<>On the other device, open DropBeam → <b>Have a code?</b> and scan this QR code — or paste the code.</>}
            footer={
              <div style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 'var(--font-sm)', color: 'var(--text-muted)' }}>
                <Spinner size={14} />
                Waiting for the other device to connect…
              </div>
            }
          />
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
            <span style={{ fontSize: 'var(--font-lg)', fontWeight: 750 }} className="gradient-text">
              {Math.round(t.percent)}%
            </span>
            <span style={{ fontSize: 'var(--font-xs)', color: 'var(--text-muted)' }}>
              {formatBytesLive(t.bytesDone)}
              {t.bytesTotal > 0 ? ` / ${formatBytesLive(t.bytesTotal)}` : ''}
            </span>
          </div>
          <ProgressBar percent={t.percent} />
          <div className="xfer-meters">
            <button
              className="xfer-meter"
              onClick={toggleSpeedMode}
              title={
                speedMode === 'live'
                  ? 'Speed over the last few seconds — click for the whole-transfer average'
                  : 'Average speed for the whole transfer — click for the live rate'
              }
            >
              {speedText}
            </button>
            <button
              className="xfer-meter"
              onClick={toggleEtaMode}
              title={
                etaMode === 'avg'
                  ? 'Based on the whole-transfer average — click to base it on the live rate'
                  : 'Based on the live rate — click to base it on the whole-transfer average'
              }
            >
              {etaText}
            </button>
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
            fontSize: 'var(--font-sm)',
            color: 'var(--text-muted)',
          }}
        >
          <Spinner size={15} />
          Connecting to {t.friendName}’s device…
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
              fontSize: 'var(--font-sm)',
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
          <div style={{ fontSize: 'var(--font-sm)', color: 'var(--text-muted)' }}>
            <div>
              {t.direction === 'receive' ? 'Saved' : 'Delivered'}
              {t.bytesTotal > 0 ? ` · ${formatBytes(t.bytesTotal)}` : ''}
            </div>
            {summary && (
              <div style={{ fontSize: 'var(--font-xs)', color: 'var(--text-faint)', marginTop: 2 }}>
                {formatEta(summary.durationMs / 1000)} · {formatSpeed(summary.avgBps)} avg
              </div>
            )}
          </div>
          {MOBILE_UI && t.direction === 'receive' && t.outDir && (
            <ShareFilesButton outDir={t.outDir} fileNames={t.fileNames} />
          )}
          {!MOBILE_UI && t.direction === 'receive' && t.outDir && (
            <button
              className="btn btn-ghost btn-sm"
              style={{ flexShrink: 0 }}
              onClick={() => {
                // A single file → reveal it SELECTED in its folder ("Show in
                // folder"); multiple → just open the folder. Match the folder's own
                // path separator so it works on Windows + macOS.
                const sep = t.outDir!.includes('\\') ? '\\' : '/'
                // A folder card carries ONE display name standing for many files
                // (fileCount), so only a genuinely single-file card reveals a file.
                if (t.fileCount === 1 && t.fileNames.length === 1) {
                  api.revealPath(`${t.outDir}${sep}${t.fileNames[0]}`).catch(() => {})
                } else {
                  api.openPath(t.outDir!).catch(() => {})
                }
              }}
            >
              <FolderOpen size={14} />{' '}
              {t.fileCount === 1 && t.fileNames.length === 1 ? 'Show in folder' : 'Open folder'}
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
                <span style={{ fontSize: 'var(--font-sm)', color: 'var(--text-muted)' }}>
                  Verifying… {t.verify.checked.toLocaleString()} /{' '}
                  {t.verify.total.toLocaleString()} files
                  {t.verify.bytesTotal > 0
                    ? ` · ${formatBytes(t.verify.bytesHashed)} / ${formatBytes(t.verify.bytesTotal)}`
                    : ''}
                </span>
                <button
                  className="btn btn-ghost btn-sm"
                  style={{ flexShrink: 0 }}
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
              style={{ display: 'flex', alignItems: 'center', gap: 8, fontSize: 'var(--font-sm)', color: 'var(--green)' }}
            >
              <CheckCircle2 size={15} /> All {t.verify.total.toLocaleString()} files identical
            </div>
          ) : t.verify?.state === 'done' ? (
            <details>
              <summary style={{ cursor: 'pointer', fontSize: 'var(--font-sm)', color: 'var(--red)' }}>
                {(t.verify.mismatched.length + t.verify.missing.length).toLocaleString()} of{' '}
                {t.verify.total.toLocaleString()} files don’t match
              </summary>
              <div
                className="selectable"
                style={{
                  marginTop: 6,
                  maxHeight: 150,
                  overflowY: 'auto',
                  fontSize: 'var(--font-xs)',
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
                <div style={{ fontSize: 'var(--font-sm)', color: 'var(--red)', marginBottom: 8, lineHeight: 1.45 }}>
                  {t.verify.error ?? 'Could not verify the copy.'}
                </div>
              )}
              {t.verify?.state === 'canceled' && (
                <div style={{ fontSize: 'var(--font-sm)', color: 'var(--text-muted)', marginBottom: 8 }}>
                  Verification canceled.
                </div>
              )}
              <div className="xfer-actions" style={{ marginTop: 0 }}>
                <button
                  className="btn btn-ghost btn-sm"
                  title="Re-hash every file on both devices and compare (SHA-256)"
                  onClick={() => {
                    void api.verifyTransfer(t.id).catch((e) => toast('error', String(e)))
                  }}
                >
                  <Check size={14} /> Verify copy
                </button>
              </div>
            </>
          )}
        </div>
      )}

      {/* failed */}
      {t.state === 'failed' && (
        <div style={{ marginTop: 12 }}>
          <div
            style={{
              fontSize: 'var(--font-sm)',
              color: 'var(--red)',
              background: 'var(--red-soft)',
              borderRadius: 'var(--radius-md)',
              padding: '10px 12px',
              lineHeight: 1.45,
            }}
          >
            {t.error ?? 'The transfer failed.'}
          </div>
          {/* One-tap re-send, only on a failed SEND (a failed receive has no original
              paths/recipient to replay — retryTransfer is a no-op there). */}
          {t.direction === 'send' && (
            <div className="xfer-actions">
              <button className="btn btn-primary btn-sm" onClick={() => void retryTransfer(t.id)}>
                <RotateCw size={14} /> Retry
              </button>
            </div>
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
              fontSize: 'var(--font-sm)',
              color: 'var(--text-muted)',
              background: 'var(--surface-2)',
              borderRadius: 'var(--radius-md)',
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
            <div className="xfer-actions">
              <button className="btn btn-primary btn-sm" onClick={() => void retryTransfer(t.id)}>
                <Play size={14} /> Resume
              </button>
            </div>
          )}
        </div>
      )}

      {t.state === 'canceled' && (
        <div style={{ marginTop: 12, fontSize: 'var(--font-sm)', color: 'var(--text-muted)' }}>
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
