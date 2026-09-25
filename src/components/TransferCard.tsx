import { FileIcon } from './FileIcon'
import { integrityLabel } from '../lib/integrity'
import { MOBILE_UI } from '../lib/platform'
import { memo, useState } from 'react'
import { motion } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import { ShareCode } from './CodeQr'
import {
  Check,
  CheckCircle2,
  FolderOpen,
  Pause,
  Play,
  RotateCw,
  ShieldCheck,
  X,
} from 'lucide-react'
import { api, isActive, type TransferUpdate } from '../lib/api'
import { formatBytes, formatBytesLive, formatEta, formatSpeed as formatSpeedValue } from '../lib/format'
import { ProgressBar, Spinner, IconButton, MenuButton } from './ui'
import { folderLabel } from '../lib/humanize'
import { IntegrityDetails } from './IntegrityDetails'
import { ConnInfo } from './ConnInspector'
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

  const failed = t.state === 'failed'
  const paused = t.state === 'paused'
  const completed = t.state === 'completed'
  const canceled = t.state === 'canceled'
  const transferring = t.state === 'transferring'
  const connecting = !isOffer && !isSendWaiting && (t.state === 'starting' || t.state === 'waitingForPeer' || t.state === 'connecting')
  const who = t.friendName
  const showInFolder = () => {
    // A single file → reveal it SELECTED in its folder; several → open the folder.
    // Match the folder's own separator so it works on Windows + macOS. A folder
    // card carries ONE display name standing for many files (fileCount).
    const sep = t.outDir!.includes('\\') ? '\\' : '/'
    if (t.fileCount === 1 && t.fileNames.length === 1) api.revealPath(`${t.outDir}${sep}${t.fileNames[0]}`).catch(() => {})
    else api.openPath(t.outDir!).catch(() => {})
  }
  const verify = t.verify
  const verifyOk = verify?.state === 'done' && verify.mismatched.length + verify.missing.length === 0
  const verifyBad = verify?.state === 'done' && !verifyOk
  const locationNotes = [
    t.locationSkipped ? `${t.locationSkipped} already there` : '',
    t.locationConflicts ? `${t.locationConflicts} kept as copies` : '',
    t.locationReplaced ? `${t.locationReplaced} updated` : '',
  ].filter(Boolean)
  const locationTip = [
    t.locationSkipped ? `${t.locationSkipped} ${t.locationSkipped === 1 ? 'file was' : 'files were'} already there.` : '',
    t.locationConflicts ? `${t.locationConflicts} ${t.locationConflicts === 1 ? 'file' : 'files'} already existed with different content — saved next to them as “… (2)”.` : '',
    t.locationReplaced ? `${t.locationReplaced} ${t.locationReplaced === 1 ? 'file was' : 'files were'} updated — the older version is in the folder’s Trash.` : '',
  ].filter(Boolean).join(' ')

  // ── the one-line status under the title ──────────────────────────────────
  let meta: React.ReactNode
  if (isOffer) {
    meta = <>{who ?? 'Someone'} wants to send you this{t.bytesTotal > 0 ? ` · ${formatBytes(t.bytesTotal)}` : ''}</>
  } else if (isSendWaiting) {
    meta = 'Waiting for the other device to scan or paste the code'
  } else if (t.detail && active && !transferring) {
    meta = t.detail
  } else if (connecting) {
    meta = <span className="xfer-connecting"><Spinner size={11} />{statusLabel(t)}</span>
  } else if (transferring) {
    meta = (
      <>
        {statusLabel(t)}
        {t.bytesTotal > 0 && <> · <span className="tnum">{formatBytesLive(t.bytesDone)} of {formatBytesLive(t.bytesTotal)}</span></>}
        {' · '}
        <button className="xfer-meter" onClick={toggleSpeedMode}
          title={speedMode === 'live' ? 'Current speed — click for the average' : 'Average speed — click for the current speed'}>
          {speedText.replace(/ (live|avg)$/, '')}
        </button>
        {' · '}
        <button className="xfer-meter" onClick={toggleEtaMode}
          title={etaMode === 'avg' ? 'Estimated from the average speed — click to use the current speed' : 'Estimated from the current speed — click to use the average'}>
          {etaText.replace(/ · (live|avg)$/, '')}
        </button>
      </>
    )
  } else if (paused) {
    meta = <>Paused{who ? ` · to ${who}` : ''}{t.bytesTotal > 0 ? <> · <span className="tnum">{formatBytes(t.bytesDone)} of {formatBytes(t.bytesTotal)}</span></> : ''}</>
  } else if (failed) {
    meta = <span className="xfer-error" title={t.error ?? undefined}>{t.direction === 'send' ? 'Couldn’t send' : 'Couldn’t receive'}{t.error ? ` — ${t.error}` : ''}</span>
  } else if (completed) {
    const saved = t.direction === 'receive' && t.outDir ? `Saved to ${folderLabel(t.outDir)}` : null
    meta = (
      <span title={summary ? `Took ${formatEta(summary.durationMs / 1000)} · ${formatSpeed(summary.avgBps)} average` : undefined}>
        {statusLabel(t)}
        {t.bytesTotal > 0 ? ` · ${formatBytes(t.bytesTotal)}` : ''}
        {saved && ` · ${saved}`}
        {locationNotes.length > 0 && <span title={locationTip}> · {locationNotes.join(' · ')}</span>}
      </span>
    )
  } else if (canceled) {
    meta = statusLabel(t)
  } else {
    meta = statusLabel(t)
  }

  return (
    <motion.div
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0, transition: { duration: 0.12 } }}
      transition={{ duration: 0.16 }}
      className={`xfer-row${canceled ? ' is-muted' : ''}`}
    >
      <div className="xfer-head">
        <span className="xfer-icon" aria-hidden>
          <FileIcon name={t.fileNames.length > 1 || t.fileCount > 1 ? (t.fileNames[0] ?? '') : (t.fileNames[0] ?? '')} size={22} />
          {(completed || failed) && (
            <span className={`xfer-badge ${completed ? 'ok' : 'bad'}`}>{completed ? <Check size={9} strokeWidth={3} /> : <X size={9} strokeWidth={3} />}</span>
          )}
        </span>
        <div className="xfer-main">
          <div className="xfer-title selectable" title={t.fileNames.join('\n') || undefined}>{title(t)}</div>
          <div className="xfer-meta">{meta}</div>
        </div>
        <div className="xfer-trailing">
          {isOffer ? (
            <>
              <button className="btn btn-secondary btn-sm" onClick={() => respondToOffer(t.id, false)}>Decline</button>
              <button className="btn btn-primary btn-sm" onClick={() => respondToOffer(t.id, true)}>Accept</button>
            </>
          ) : (
            <>
              {t.detail && t.state === 'waitingForPeer' && (
                <button className="btn btn-plain btn-sm" onClick={() => void api.forceRelay(t.id)} title="Send through the relay now instead of waiting for a direct connection">
                  Send Anyway
                </button>
              )}
              {failed && t.direction === 'send' && (
                <button className="btn btn-secondary btn-sm" onClick={() => void retryTransfer(t.id)}>
                  <RotateCw /> Retry
                </button>
              )}
              {paused && t.direction === 'send' && (
                <button className="btn btn-secondary btn-sm" onClick={() => void retryTransfer(t.id)}>
                  <Play /> Resume
                </button>
              )}
              {(transferring || connecting) && <ConnInfo detail={t.connDetail} locality={t.locality} />}
              {completed && t.direction === 'receive' && t.outDir && (
                <IconButton label={t.fileCount === 1 && t.fileNames.length === 1 ? 'Show in Finder' : 'Open Folder'} onClick={showInFolder}>
                  <FolderOpen />
                </IconButton>
              )}
              {completed && t.direction === 'send' && verify?.state !== 'running' && (
                <MenuButton
                  items={[
                    { label: verify ? 'Verify Again' : 'Verify Copy', icon: <ShieldCheck />, onSelect: () => void api.verifyTransfer(t.id).catch((e) => toast('error', String(e))) },
                  ]}
                />
              )}
              {canPause && (
                <IconButton label="Pause" tooltip="Pause — keeps what’s already sent" onClick={() => void api.pauseTransfer(t.id)}>
                  <Pause fill="currentColor" strokeWidth={0} />
                </IconButton>
              )}
              <IconButton
                label={active ? 'Cancel' : 'Remove from list'}
                onClick={() => (active ? api.cancelTransfer(t.id) : removeTransfer(t.id))}
              >
                <X />
              </IconButton>
            </>
          )}
        </div>
      </div>

      {(transferring || (paused && t.bytesTotal > 0)) && (
        <div className="xfer-progress">
          <ProgressBar percent={t.percent} tone={paused ? 'paused' : undefined} label={`${title(t)} progress`} />
        </div>
      )}

      {isSendWaiting && (
        <div className="xfer-code">
          <ShareCode
            code={t.code!}
            size={132}
            hint={null}
            copyVariant="secondary"
            instructions="On the other device, open DropBeam, choose Receive, and scan this code — or paste it."
            footer={<span className="xfer-connecting"><Spinner size={11} />Waiting for the other device…</span>}
          />
        </div>
      )}

      {completed && t.direction === 'send' && verify && (
        <div className="xfer-verify">
          {verify.state === 'running' ? (
            <>
              <div className="xfer-verify-line">
                <span className="tnum">Verifying… {verify.checked.toLocaleString()} of {verify.total.toLocaleString()} {verify.total === 1 ? 'file' : 'files'}</span>
                <button className="btn btn-plain btn-sm" onClick={() => void api.cancelVerify(t.id)}>Stop</button>
              </div>
              <ProgressBar percent={verify.bytesTotal > 0 ? (verify.bytesHashed / verify.bytesTotal) * 100 : 0} label="Verification progress" />
            </>
          ) : verifyOk ? (
            <div className="xfer-verify-line ok"><CheckCircle2 size={13} /> {verify.total === 1 ? 'The copy matches' : `All ${verify.total.toLocaleString()} files match`}</div>
          ) : verifyBad ? (
            <details className="xfer-verify-bad">
              <summary>{(verify.mismatched.length + verify.missing.length).toLocaleString()} of {verify.total.toLocaleString()} files don’t match</summary>
              <div className="selectable">
                {verify.mismatched.map((name) => <div key={`different:${name}`}>Different: {name}</div>)}
                {verify.missing.map((name) => <div key={`missing:${name}`}>Missing: {name}</div>)}
              </div>
            </details>
          ) : verify.state === 'failed' ? (
            <div className="xfer-verify-line bad">{verify.error ?? 'Couldn’t verify the copy.'}</div>
          ) : null}
        </div>
      )}

      <IntegrityDetails rows={t.integrity} total={t.bytesTotal} completed={completed} />
    </motion.div>
  )
}

function statusLabel(t: TransferUpdate): string {
  const fn = t.friendName
  const send = t.direction === 'send'
  switch (t.state) {
    case 'starting':
      return send && fn ? `Connecting to ${fn}…` : 'Starting…'
    case 'waitingForPeer':
      return send && fn ? `Waiting for ${fn}…` : 'Ready to send'
    case 'connecting':
      return fn ? (send ? `Connecting to ${fn}…` : `Connecting to ${fn}…`) : 'Connecting…'
    case 'waitingForAccept':
      return fn ? `${fn} wants to send files` : 'Incoming files'
    case 'transferring':
      return send ? (fn ? `Sending to ${fn}` : 'Sending') : fn ? `Receiving from ${fn}` : 'Receiving'
    case 'completed':
      return send ? (fn ? `Sent to ${fn}` : 'Sent') : fn ? `Received from ${fn}` : 'Received'
    case 'failed':
      return send ? 'Couldn’t send' : 'Couldn’t receive'
    case 'canceled':
      return !send && fn ? 'Declined' : 'Canceled'
    case 'paused':
      return 'Paused'
  }
}
