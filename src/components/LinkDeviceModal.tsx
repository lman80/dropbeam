import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { AlertCircle, CheckCircle2, X } from 'lucide-react'
import { listen } from '@tauri-apps/api/event'
import { api, HAS_TAURI } from '../lib/api'
import { mockListen } from '../lib/mock'
import { MOBILE_UI } from '../lib/platform'
import { deviceCodeProblem, isDeviceCode, linkedDetail, linkedTitle, linkErrorText, progressText, type LinkedDevice, type LinkProgress } from '../lib/deviceLink'
import { useStore } from '../store'
import { QrCodeView } from './CodeQr'
import { QrScanner } from './QrScanner'
import { useEscape } from './Dialog'
import { IconButton, Spinner } from './ui'

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

  if (phase === 'scan') return <QrScanner title="Scan your other device" hint="On your other device, open Settings → Devices → Link a device."
    validate={deviceCodeProblem} onResult={v => void scanned(v)} onClose={() => start === 'scan' ? closeRef.current() : setPhase('show')} />

  const footer = phase === 'show' ? <>
    <button className="btn btn-plain" onClick={() => setPhase('scan')}>Scan their code instead</button>
    <span className="spacer" />
    {codeError
      ? <button className="btn btn-secondary" onClick={retry}>Try again</button>
      : <button className="btn btn-secondary" disabled={!code} onClick={copy}>{copied ? 'Copied' : 'Copy code'}</button>}
    <button className="btn btn-secondary" onClick={onClose}>Cancel</button>
  </> : phase === 'working' ? <button className="btn btn-secondary" onClick={onClose}>Hide</button>
    : phase === 'done' ? <button className="btn btn-primary" autoFocus onClick={onClose}>Done</button>
    : <>
      {via === 'scan'
        ? <button className="btn btn-plain" onClick={() => { setError(''); setPhase('show') }}>Show this device’s code</button>
        : <button className="btn btn-plain" onClick={() => { setError(''); setPhase('scan') }}>Scan their code instead</button>}
      <span className="spacer" />
      <button className="btn btn-secondary" onClick={onClose}>Close</button>
      <button className="btn btn-primary" autoFocus onClick={retry}>Try again</button>
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
  return createPortal(<div className="dialog-overlay device-link-overlay" onMouseDown={e => { if (onClose && e.target === e.currentTarget) onClose() }}>
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
