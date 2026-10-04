import { useCallback, useEffect, useLayoutEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { AlertCircle, CheckCircle2, X } from 'lucide-react'
import { listen } from '@tauri-apps/api/event'
import { api, HAS_TAURI, type LinkConfirmRequest, type LinkPreviewInfo } from '../lib/api'
import { mockListen } from '../lib/mock'
import { MOBILE_UI } from '../lib/platform'
import { deviceCodeProblem, isDeviceCode, linkedDetail, linkedTitle, linkErrorText, progressText, type LinkedDevice, type LinkProgress } from '../lib/deviceLink'
import { useStore } from '../store'
import { QrCodeView } from './CodeQr'
import { QrScanner } from './QrScanner'
import { useEscape, useModalFocus } from './Dialog'
import { IconButton, Spinner } from './ui'

/** Link with a scanned/pasted device code of either kind. The engine picks the
 *  direction (the account that already has devices wins) and refuses two
 *  different accounts, so both commands take any device code. */
// eslint-disable-next-line react-refresh/only-export-components -- shared helper
export async function linkWithCode(code: string, confirm: string) {
  const c = code.trim()
  const problem = deviceCodeProblem(c)
  if (problem) throw new Error(problem)
  return /^dropbeamjoin1:/i.test(c) ? api.linkDeviceJoin(c, confirm) : api.linkDeviceSend(c, confirm)
}
// eslint-disable-next-line react-refresh/only-export-components -- shared helper
export { isDeviceCode }

function onLinkEvent<T>(name: string, cb: (payload: T) => void): Promise<() => void> {
  if (!HAS_TAURI) return mockListen(name, p => cb(p as T))
  return listen<T>(name, e => cb(e.payload))
}

// 'confirm': this device scanned a code — check the safety code, then link.
// 'incoming': the other device scanned OURS — check the safety code it shows.
type Phase = 'show' | 'scan' | 'working' | 'confirm' | 'incoming' | 'done' | 'error'

/**
 * The one linking flow, whichever device it's opened on. This device shows
 * its code AND can scan the other's — either device scanning the other works,
 * and the account that already has devices is the one both end up in.
 * `start`: open on the code ('show') or straight in the camera ('scan').
 */
export function LinkFlow({ onClose, start, title }: { onClose: () => void; start: 'show' | 'scan'; title: string }) {
  const myDevice = useStore(s => s.myDevice)
  // A device that already shares its account shows a "join me" code (older
  // builds scanning it then join); a new one shows "link me".
  const hosting = (myDevice?.devices?.length ?? 0) > 1
  const [phase, setPhase] = useState<Phase>(start)
  const [attempt, setAttempt] = useState(0)
  const [code, setCode] = useState('')
  const [codeError, setCodeError] = useState('')
  const [progress, setProgress] = useState<LinkProgress | null>(null)
  const [linked, setLinked] = useState<LinkedDevice | null>(null)
  const [error, setError] = useState('')
  const [copied, setCopied] = useState(false)
  // The safety-code step (S1): what this device scanned, or who scanned us.
  const [preview, setPreview] = useState<{ code: string; info: LinkPreviewInfo } | null>(null)
  const [incoming, setIncoming] = useState<LinkConfirmRequest | null>(null)
  // How the last attempt was made (this device scanning, or showing its code).
  const [via, setVia] = useState<'show' | 'scan'>(start)
  const phaseRef = useRef(phase)
  useLayoutEffect(() => { phaseRef.current = phase }, [phase])
  const closeRef = useRef(onClose)
  useEffect(() => { closeRef.current = onClose }, [onClose])

  const finished = useCallback((d: LinkedDevice | null) => {
    setLinked(d); setPhase('done'); setProgress(null)
    void useStore.getState().reloadFriends().catch(() => {})
  }, [])

  // The other device scanned OUR code: its progress, success and failure arrive as events.
  useEffect(() => {
    let alive = true
    const stops: (() => void)[] = []
    void (async () => {
      const add = async (p: Promise<() => void>) => { const stop = await p.catch(() => undefined); if (!stop) return; if (alive) stops.push(stop); else stop() }
      await add(onLinkEvent<LinkProgress>('link://progress', p => {
        if (!alive || phaseRef.current === 'done') return
        setProgress(p)
        if (phaseRef.current === 'show') { setVia('show'); setPhase('working') }
      }))
      await add(onLinkEvent<LinkedDevice>('link://linked', d => { if (alive && phaseRef.current !== 'done') finished(d) }))
      await add(onLinkEvent<LinkConfirmRequest>('link://confirm', r => {
        if (!alive || phaseRef.current === 'done') return
        setIncoming(r); setVia('show'); setPhase('incoming')
      }))
      await add(onLinkEvent<string>('link://failed', e => {
        if (!alive || phaseRef.current === 'done') return
        setError(linkErrorText(e)); setPhase('error')
      }))
    })()
    return () => { alive = false; stops.forEach(s => s()) }
  }, [finished])

  // A fresh one-time code each time the code screen opens (or on Try again).
  const showing = phase === 'show'
  useEffect(() => {
    if (!showing) return
    let alive = true
    // eslint-disable-next-line react-hooks/set-state-in-effect -- a fresh code screen starts blank
    setCode(''); setCodeError(''); setCopied(false)
    const begin = hosting ? api.linkHostBegin : api.linkDeviceBegin
    begin().then(c => { if (alive) setCode(c) }).catch(e => { if (alive) setCodeError(`Couldn’t create a code. ${linkErrorText(e)}`) })
    return () => { alive = false }
  }, [showing, attempt, hosting])
  // Whatever happens, no code stays live after the dialog closes.
  useEffect(() => () => { void api.linkHostCancel().catch(() => {}); void api.linkDeviceCancel().catch(() => {}) }, [])

  // Scanned the other device's code: it shows the safety code now, and this
  // device shows the same one — nothing moves until the user confirms (S1).
  const scanned = async (value: string) => {
    setPhase('working'); setVia('scan'); setProgress(null); setError(''); setPreview(null)
    try {
      const problem = deviceCodeProblem(value.trim())
      if (problem) throw new Error(problem)
      const info = await api.linkDevicePrepare(value.trim())
      if (phaseRef.current === 'working') { setPreview({ code: value.trim(), info }); setPhase('confirm') }
    } catch (e) { if (phaseRef.current !== 'done') { setError(linkErrorText(e)); setPhase('error') } }
  }
  const confirmScanned = async () => {
    if (!preview) return
    setPhase('working')
    try { finished(await linkWithCode(preview.code, preview.info.confirmToken)) }
    catch (e) { if (phaseRef.current !== 'done') { setError(linkErrorText(e)); setPhase('error') } }
  }
  const answerIncoming = (accept: boolean) => {
    const r = incoming
    if (!r) return
    void api.linkConfirm(r.endpointId, accept).catch(() => {})
    if (accept) { setProgress(null); setPhase('working') } else onClose()
  }
  const retry = () => { setError(''); setProgress(null); if (via === 'scan') setPhase('scan'); else { setAttempt(a => a + 1); setPhase('show') } }
  const copy = () => void navigator.clipboard.writeText(code).then(() => setCopied(true)).catch(() => setCodeError('Couldn’t copy the code. Select it and copy it instead.'))

  if (phase === 'scan') return <QrScanner title="Scan your other device" hint="On your other device, open Settings → Devices → Link a device."
    validate={deviceCodeProblem} onResult={v => void scanned(v)} onClose={() => start === 'scan' ? closeRef.current() : setPhase('show')} />

  const footer = phase === 'confirm' ? <>
    <span className="spacer" />
    <button className="btn btn-secondary" onClick={onClose}>Cancel</button>
    <button className="btn btn-primary" autoFocus onClick={() => void confirmScanned()}>Codes Match — Link</button>
  </> : phase === 'incoming' ? <>
    <span className="spacer" />
    <button className="btn btn-secondary" onClick={() => answerIncoming(false)}>Cancel</button>
    <button className="btn btn-primary" autoFocus onClick={() => answerIncoming(true)}>Codes Match — Link</button>
  </> : phase === 'show' ? <>
    <button className="btn btn-plain" onClick={() => setPhase('scan')}>Scan Their Code Instead</button>
    <span className="spacer" />
    {codeError
      ? <button className="btn btn-secondary" onClick={retry}>Try Again</button>
      : <button className="btn btn-secondary" disabled={!code} onClick={copy}>{copied ? 'Copied' : 'Copy Code'}</button>}
    <button className="btn btn-secondary" onClick={onClose}>Cancel</button>
  </> : phase === 'working' ? <button className="btn btn-secondary" onClick={onClose}>Cancel Linking</button>
    : phase === 'done' ? <button className="btn btn-primary" autoFocus onClick={onClose}>Done</button>
    : <>
      {via === 'scan'
        ? <button className="btn btn-plain" onClick={() => { setError(''); setPhase('show') }}>Show This Device’s Code</button>
        : <button className="btn btn-plain" onClick={() => { setError(''); setPhase('scan') }}>Scan Their Code Instead</button>}
      <span className="spacer" />
      <button className="btn btn-secondary" onClick={onClose}>Close</button>
      <button className="btn btn-primary" autoFocus onClick={retry}>Try Again</button>
    </>

  return <LinkDialog title={phase === 'done' ? 'Devices linked' : title} onClose={onClose} footer={footer}>
    {phase === 'show' && <div className="link-show">
      <p className="link-instruction">On your other device, open <strong>Settings → Devices → Link a device</strong> and scan this code.</p>
      <div className="link-qr-slot">
        {code ? <QrCodeView value={code} size={200} hint={null} label="QR code to link your other device" />
          : !codeError && <Spinner size={18} />}
      </div>
      {codeError
        ? <p role="alert" className="form-error link-status">{codeError}</p>
        : <p className="link-status" role="status">{code ? 'Waiting for your other device…' : 'Creating a code…'}</p>}
    </div>}
    {phase === 'confirm' && preview && <div className="link-state" role="alertdialog" aria-label="Check the safety code">
      <p className="link-state-sub">Make sure <strong>{preview.info.name}</strong> shows this code:</p>
      <p className="link-state-title link-safety-code" aria-live="polite">{preview.info.safety}</p>
      <p className="link-state-sub">{preview.info.direction === 'give'
        ? `${preview.info.name} will get full access to your account: your friends, chats and devices.`
        : `This device will join ${preview.info.name}’s account: this device’s friends and chats will be shared into that account, and it gets that account’s friends and chats.`}
        {' '}Only continue if this is your own device{preview.info.peerShowsCode ? ' and the codes match' : ' (it needs an update to show the code)'}.</p>
    </div>}
    {phase === 'incoming' && incoming && <div className="link-state" role="alertdialog" aria-label="Check the safety code">
      <p className="link-state-sub"><strong>{incoming.name}</strong> wants to link with this device. Make sure it shows this code:</p>
      <p className="link-state-title link-safety-code" aria-live="polite">{incoming.safety}</p>
      <p className="link-state-sub">{incoming.joining
        ? `This device will join ${incoming.name}’s account: this device’s friends and chats will be shared into that account, and it gets that account’s friends and chats.`
        : `${incoming.name} will get full access to your account: your friends, chats and devices.`}
        {' '}Only continue if it’s your own device and the codes match.</p>
    </div>}
    {phase === 'working' && <div className="link-state" role="status" aria-live="polite">
      <Spinner size={22} />
      <p className="link-state-title">{progressText(progress)}</p>
      <p className="link-state-sub">Keep DropBeam open on both devices.</p>
    </div>}
    {phase === 'done' && <div className="link-state" role="status">
      <CheckCircle2 className="link-state-ok" size={30} strokeWidth={1.75} />
      <p className="link-state-title">{linkedTitle(linked)}</p>
      <p className="link-state-sub">{linkedDetail(linked)}</p>
    </div>}
    {phase === 'error' && <div className="link-state" role="alert">
      <AlertCircle className="link-state-bad" size={30} strokeWidth={1.75} />
      <p className="link-state-title">Couldn’t link</p>
      <p className="link-state-sub">{error}</p>
    </div>}
  </LinkDialog>
}

/** Show this device's code (a new device linking to an account you already use). */
export function LinkDeviceModal({ onClose }: { onClose: () => void }) {
  return <LinkFlow start="show" title="Link this device" onClose={onClose} />
}

/** Scan a new device's code from a device you already use. */
export function LinkNewDeviceModal({ onClose }: { onClose: () => void }) {
  return <LinkFlow start="scan" title="Link a new device" onClose={onClose} />
}

function LinkDialog({ title, onClose, footer, children }: { title: string; onClose?: () => void; footer?: React.ReactNode; children: React.ReactNode }) {
  // Stacks with the scanner and any dialog that opened this one: Esc peels the topmost.
  useEscape(onClose)
  const ref = useRef<HTMLDivElement>(null)
  useModalFocus(ref)
  return createPortal(<div ref={ref} className="dialog-overlay device-link-overlay" onMouseDown={e => { if (onClose && e.target === e.currentTarget) onClose() }}>
    <div className={MOBILE_UI ? 'dialog mobile-sheet device-link-dialog' : 'dialog dialog-panel device-link-dialog'} role="dialog" aria-modal="true" aria-label={title}>
      <div className="dialog-head">
        <h2 className="dialog-title">{title}</h2>
        {onClose && <IconButton label="Close" tooltip="Close (Esc)" onClick={onClose}><X /></IconButton>}
      </div>
      <div className="dialog-body">{children}</div>
      {footer && <div className="dialog-actions dialog-footer">{footer}</div>}
    </div>
  </div>, document.body)
}
