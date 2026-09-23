import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { AlertCircle, CheckCircle2, Copy, Loader2, QrCode, Smartphone } from 'lucide-react'
import { listen } from '@tauri-apps/api/event'
import { api, HAS_TAURI } from '../lib/api'
import { mockListen } from '../lib/mock'
import { MOBILE_UI } from '../lib/platform'
import { deviceCodeProblem, isDeviceCode, linkedDetail, linkedTitle, linkErrorText, progressText, type LinkedDevice, type LinkProgress } from '../lib/deviceLink'
import { useStore } from '../store'
import { QrCodeView } from './CodeQr'
import { QrScanner } from './QrScanner'

/** Link with a scanned/pasted device code of either kind. The engine picks the
 *  direction (the account that already has devices wins) and refuses two
 *  different accounts, so both commands take any device code. */
export async function linkWithCode(code: string) {
  const c = code.trim()
  const problem = deviceCodeProblem(c)
  if (problem) throw new Error(problem)
  return /^dropbeamjoin1:/i.test(c) ? api.linkDeviceJoin(c) : api.linkDeviceSend(c)
}
export { isDeviceCode }

function onLinkEvent<T>(name: string, cb: (payload: T) => void): Promise<() => void> {
  if (!HAS_TAURI) return mockListen(name, p => cb(p as T))
  return listen<T>(name, e => cb(e.payload))
}

type Phase = 'show' | 'scan' | 'working' | 'done' | 'error'

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
  // How the last attempt was made (this device scanning, or showing its code).
  const [via, setVia] = useState<'show' | 'scan'>(start)
  const phaseRef = useRef(phase)
  phaseRef.current = phase
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
    setCode(''); setCodeError(''); setCopied(false)
    const begin = hosting ? api.linkHostBegin : api.linkDeviceBegin
    begin().then(c => { if (alive) setCode(c) }).catch(e => { if (alive) setCodeError(`Couldn’t create a code. ${linkErrorText(e)}`) })
    return () => { alive = false }
  }, [showing, attempt, hosting])
  // Whatever happens, no code stays live after the dialog closes.
  useEffect(() => () => { void api.linkHostCancel().catch(() => {}); void api.linkDeviceCancel().catch(() => {}) }, [])

  const scanned = async (value: string) => {
    setPhase('working'); setVia('scan'); setProgress(null); setError('')
    try { finished(await linkWithCode(value)) }
    catch (e) { if (phaseRef.current !== 'done') { setError(linkErrorText(e)); setPhase('error') } }
  }
  const retry = () => { setError(''); setProgress(null); if (via === 'scan') setPhase('scan'); else { setAttempt(a => a + 1); setPhase('show') } }
  const copy = () => void navigator.clipboard.writeText(code).then(() => setCopied(true)).catch(() => setCodeError('Couldn’t copy the code. Select it and copy it instead.'))

  if (phase === 'scan') return <QrScanner title="Scan your other device" hint="On your other device open Settings → Devices → Link a Device, then scan the code it shows."
    validate={deviceCodeProblem} onResult={v => void scanned(v)} onClose={() => start === 'scan' ? closeRef.current() : setPhase('show')} />

  return <LinkDialog title={phase === 'done' ? 'Devices linked' : title} onClose={onClose}>
    {phase === 'show' && <>
      <ol className="device-link-steps">
        <li>Open DropBeam on your other device.</li>
        <li>Go to <strong>Settings → Devices → Link a Device</strong> — on a phone you’re just setting up, tap <strong>Already use DropBeam?</strong></li>
        <li>Scan this code.</li>
      </ol>
      {code ? <QrCodeView value={code} size={220} hint="Scan with DropBeam on your other device" label="QR code to link your other device" />
        : !codeError && <p className="account-waiting" role="status"><Loader2 size={14} className="spin" /> Creating a code…</p>}
      {codeError && <p role="alert" className="error-text">{codeError}</p>}
      {code && <p className="account-waiting" role="status">Waiting for your other device… The code works once, for 10 minutes.</p>}
      <div className="device-link-actions">
        <button className="btn btn-ghost" onClick={() => setPhase('scan')}><QrCode size={14} />Scan the other device’s code instead</button>
        {code && <button className="btn btn-ghost" onClick={copy}><Copy size={14} />{copied ? 'Copied' : 'Copy code'}</button>}
        {codeError && <button className="btn btn-ghost" onClick={retry}>Try again</button>}
        <button className="btn btn-ghost" onClick={onClose}>Cancel</button>
      </div>
      <p className="device-link-note">Either device can scan the other. Your friends and chats come along, and nothing on either device is lost.</p>
    </>}
    {phase === 'working' && <div className="device-link-state" role="status" aria-live="polite">
      <Loader2 size={34} className="spin" />
      <p><strong>{progressText(progress)}</strong></p>
      <p className="account-waiting">Keep DropBeam open on both devices.</p>
      <button className="btn btn-ghost" onClick={onClose}>Hide</button>
    </div>}
    {phase === 'done' && <div className="account-linked" role="status">
      <CheckCircle2 size={40} />
      <p><strong>{linkedTitle(linked)}</strong></p>
      <p>{linkedDetail(linked)}</p>
      <button className="btn btn-primary" onClick={onClose}>Done</button>
    </div>}
    {phase === 'error' && <div className="device-link-state device-link-error" role="alert">
      <AlertCircle size={34} />
      <p>{error}</p>
      <div className="device-link-actions">
        <button className="btn btn-primary" onClick={retry}>Try again</button>
        {via === 'scan'
          ? <button className="btn btn-ghost" onClick={() => { setError(''); setPhase('show') }}><Smartphone size={14} />Show this device’s code instead</button>
          : <button className="btn btn-ghost" onClick={() => { setError(''); setPhase('scan') }}><QrCode size={14} />Scan the other device instead</button>}
        <button className="btn btn-ghost" onClick={onClose}>Close</button>
      </div>
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

function LinkDialog({ title, onClose, children }: { title: string; onClose?: () => void; children: React.ReactNode }) {
  useEffect(() => {
    if (!onClose) return
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  return createPortal(<div className="dialog-overlay device-link-overlay" onMouseDown={e => { if (onClose && e.target === e.currentTarget) onClose() }}>
    <div className={MOBILE_UI ? 'dialog mobile-sheet device-link-dialog' : 'card dialog device-link-dialog'} role="dialog" aria-modal="true" aria-label={title}><h2>{title}</h2>{children}</div>
  </div>, document.body)
}
