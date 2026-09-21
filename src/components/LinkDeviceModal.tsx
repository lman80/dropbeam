import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { listen } from '@tauri-apps/api/event'
import { QRCodeSVG } from 'qrcode.react'
import { api, HAS_TAURI, onFriendsChanged } from '../lib/api'
import { MOBILE_UI } from '../lib/platform'
import { useStore } from '../store'
import { QrScanner } from './QrScanner'

export function LinkNewDeviceModal({ onClose }: { onClose: () => void }) {
  const [scanning, setScanning] = useState(true)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const send = async (code: string) => {
    setScanning(false)
    if (!/^dropbeamlink1:/i.test(code)) { setError('This is not a device linking code. Open the linking screen on your new device.'); return }
    setBusy(true)
    try {
      const linked = await api.linkDeviceSend(code)
      useStore.getState().toast('success', `Linked ${linked.name}`)
      void useStore.getState().reloadFriends().catch(() => {})
      onClose()
    } catch (e) { setError(`Could not link the device: ${String(e)}`) }
    finally { setBusy(false) }
  }
  if (scanning) return <QrScanner hint="Scan the code shown on your new device." onResult={code => void send(code)} onClose={onClose} />
  return <LinkDialog title="Link a new device"><p role="status">{busy ? 'Linking device…' : error}</p>{!busy && <><button className="btn btn-primary" onClick={() => { setError(''); setScanning(true) }}>Try again</button><button className="btn btn-ghost" onClick={onClose}>Close</button></>}</LinkDialog>
}

export function LinkDeviceModal({ onClose }: { onClose: () => void }) {
  const [code, setCode] = useState('')
  const [error, setError] = useState('')
  const [copied, setCopied] = useState(false)
  const [canceling, setCanceling] = useState(false)
  const closeRef = useRef(onClose)
  useEffect(() => { closeRef.current = onClose }, [onClose])
  const cancelRef = useRef<() => Promise<void>>(async () => {})
  useEffect(() => {
    let alive = true, begun = false, complete = false
    let unlisten: (() => void) | undefined
    const changed = () => {
      if (!alive || !begun || complete) return
      complete = true
      useStore.getState().toast('success', 'This device is now linked')
      void useStore.getState().reloadFriends().catch(() => {})
      closeRef.current()
    }
    // Subscribe to the precise completion event before displaying the code.
    const start = async () => {
      unlisten = await (HAS_TAURI ? listen('friends://changed', changed) : onFriendsChanged(changed))
      if (!alive) { unlisten(); return }
      const value = await api.linkDeviceBegin()
      begun = true
      if (!alive) { await api.linkDeviceCancel(); return }
      setCode(value)
    }
    const pending = start().catch(e => { if (alive) setError(`Could not create a linking code: ${String(e)}`) })
    cancelRef.current = async () => {
      setCanceling(true)
      await pending
      try { await api.linkDeviceCancel(); complete = true; closeRef.current() }
      catch (e) { setError(`Could not cancel linking: ${String(e)}`); setCanceling(false) }
    }
    return () => { alive = false; unlisten?.(); if (begun && !complete) void api.linkDeviceCancel().catch(() => {}) }
  }, [])
  return <LinkDialog title="Link this device to my account">
    <p>On the device you already use, open Settings &gt; Devices &gt; Link a new device and scan this code.</p>
    {code ? <><div className="link-qr"><QRCodeSVG value={code} size={240} level="M" /></div><textarea className="input" aria-label="Device linking code" readOnly value={code} /><button className="btn btn-primary" onClick={() => void navigator.clipboard.writeText(code).then(() => setCopied(true)).catch(() => setError('Could not copy the code. Select and copy the text instead.'))}>{copied ? 'Copied' : 'Copy'}</button></> : !error && <p role="status">Creating code…</p>}
    {error && <p role="alert">{error}</p>}
    <button className="btn btn-ghost" disabled={canceling} onClick={() => void cancelRef.current()}>{canceling ? 'Canceling…' : 'Cancel'}</button>
  </LinkDialog>
}

function LinkDialog({ title, children }: { title: string; children: React.ReactNode }) {
  return createPortal(<div className="dialog-overlay device-link-overlay"><div className={MOBILE_UI ? 'dialog mobile-sheet device-link-dialog' : 'card dialog device-link-dialog'} role="dialog" aria-modal="true" aria-label={title}><h2>{title}</h2>{children}</div></div>, document.body)
}
