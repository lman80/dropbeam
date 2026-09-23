import { useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import { QRCodeSVG } from 'qrcode.react'
import { CheckCircle2, MoreHorizontal, Plus, QrCode, RefreshCw } from 'lucide-react'
import { api, HAS_TAURI, type AccountDevice } from '../lib/api'
import { deviceIcon, deviceNoun } from '../lib/deviceIcons'
import { friendOnlineState } from '../lib/presence'
import { useStore } from '../store'
import { QrScanner } from './QrScanner'
import { LinkDeviceModal } from './LinkDeviceModal'

/** Link with either device code: "dropbeamjoin1:" (a device that has the
 *  account) makes THIS device join it; "dropbeamlink1:" (a new device) joins ours. */
export async function linkWithCode(code: string) {
  const c = code.trim()
  if (/^dropbeamjoin1:/i.test(c)) return api.linkDeviceJoin(c)
  if (/^dropbeamlink1:/i.test(c)) return api.linkDeviceSend(c)
  throw new Error('That isn’t a DropBeam device code. On your other device open Settings → Devices.')
}

export function isDeviceCode(code: string) { return /^dropbeam(join|link)1:/i.test(code.trim()) }

function ago(ms: number) {
  const s = Math.max(0, (Date.now() - ms) / 1000)
  if (s < 60) return 'just now'
  if (s < 3600) return `${Math.floor(s / 60)} min ago`
  if (s < 86400) return `${Math.floor(s / 3600)} hr ago`
  return new Date(ms).toLocaleDateString()
}

function DeviceGlyph({ d, size = 20 }: { d: Pick<AccountDevice, 'device_kind' | 'device_os'>; size?: number }) {
  const Icon = deviceIcon(d.device_os === 'macos' && d.device_kind !== 'desktop' ? 'laptop' : d.device_kind ?? undefined)
  return <span className="account-device-glyph"><Icon size={size} strokeWidth={1.7} aria-hidden /></span>
}

/** Settings → Devices on desktop: every device in this account, kept in sync peer to peer. */
export function DevicesPanel() {
  const myDevice = useStore(s => s.myDevice)
  const friends = useStore(s => s.friends)
  const friendSeen = useStore(s => s.friendSeen)
  const folderStatuses = useStore(s => s.folderStatuses)
  const [mode, setMode] = useState<null | 'add' | 'join' | 'show'>(null)
  const [menu, setMenu] = useState<string | null>(null)
  const [syncing, setSyncing] = useState(false)
  const devices = myDevice?.devices ?? []
  const inAccount = !!myDevice?.account_pub && devices.length > 1
  const me = devices.find(d => d.this_device)
  useEffect(() => { void useStore.getState().refreshMyDevice().catch(() => {}) }, [])
  const online = (d: AccountDevice) => { const f = friends.find(x => x.id === d.friend_id); return f ? friendOnlineState(f.name, friendSeen, folderStatuses) === true : false }
  const syncNow = async () => {
    setSyncing(true)
    try { await api.accountSyncNow(); await new Promise(r => setTimeout(r, 2500)); await useStore.getState().refreshMyDevice() } finally { setSyncing(false) }
  }
  const remove = async (d: AccountDevice) => {
    setMenu(null)
    if (!window.confirm(`Remove ${d.name} from your account? It stops syncing your friends and chats. You can link it again later.`)) return
    try { await api.accountRemoveDevice(d.endpoint_id); useStore.getState().toast('success', `${d.name} removed`); await useStore.getState().reloadFriends() }
    catch (e) { useStore.getState().toast('error', String(e)) }
  }
  const leave = async () => {
    if (!window.confirm(`Remove this ${deviceNoun(me?.device_kind, me?.device_os)} from your account? Friends and chats stay here but stop syncing with your other devices.`)) return
    try { await api.accountLeave(); useStore.getState().toast('success', 'This device left your account'); await useStore.getState().reloadFriends() }
    catch (e) { useStore.getState().toast('error', String(e)) }
  }
  return <section className="card account-devices" style={{ padding: 18, marginBottom: 16 }}>
    <div className="account-devices-head">
      <div>
        <h2>Devices</h2>
        <p className="account-devices-sub">Friends and chats sync directly between your devices — end-to-end encrypted, no cloud.</p>
      </div>
      {inAccount && <button className="btn btn-ghost" disabled={syncing} onClick={() => void syncNow()}><RefreshCw size={14} className={syncing ? 'spin' : ''} />{syncing ? 'Syncing…' : 'Sync now'}</button>}
    </div>
    {inAccount ? <>
      <ul className="account-device-list">
        {devices.map(d => {
          const noun = deviceNoun(d.device_kind, d.device_os)
          const sub = d.this_device ? d.name : [d.name, online(d) ? 'Online' : 'Offline', d.last_sync_ms ? `Synced ${ago(d.last_sync_ms)}` : null].filter(Boolean).join(' · ')
          return <li key={d.endpoint_id} className="account-device">
            <DeviceGlyph d={d} />
            <span className="account-device-text"><strong>{d.this_device ? `This ${noun}` : `Your ${noun}`}</strong><span>{sub}</span></span>
            {!d.this_device && <span className="account-device-menu">
              <button className="icon-btn" aria-label={`Options for ${d.name}`} onClick={() => setMenu(menu === d.endpoint_id ? null : d.endpoint_id)}><MoreHorizontal size={16} /></button>
              {menu === d.endpoint_id && <div className="account-device-popover" role="menu"><button role="menuitem" className="danger" onClick={() => void remove(d)}>Remove from account</button></div>}
            </span>}
          </li>
        })}
      </ul>
      <div className="device-link-actions">
        <button className="btn btn-primary" onClick={() => setMode('add')}><Plus size={14} />Add a device</button>
        <button className="btn btn-ghost danger-text" onClick={() => void leave()}>Remove this {deviceNoun(me?.device_kind, me?.device_os)} from account</button>
      </div>
    </> : <div className="account-devices-empty">
      <p><strong>Use DropBeam on your phone too?</strong> Add it here and it gets your friends and chats right away — then everything stays in sync.</p>
      <div className="device-link-actions">
        <button className="btn btn-primary" onClick={() => setMode('add')}><Plus size={14} />Add a device</button>
        <button className="btn btn-ghost" onClick={() => setMode('join')}><QrCode size={14} />Join my other device’s account</button>
      </div>
    </div>}
    {mode === 'add' && <AddDeviceModal onClose={() => setMode(null)} />}
    {mode === 'join' && <JoinAccountModal onClose={() => setMode(null)} onShowCode={() => setMode('show')} />}
    {mode === 'show' && <LinkDeviceModal onClose={() => setMode(null)} />}
  </section>
}

function Dialog({ title, onClose, children }: { title: string; onClose: () => void; children: React.ReactNode }) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') onClose() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [onClose])
  return createPortal(<div className="dialog-overlay device-link-overlay" onMouseDown={e => { if (e.target === e.currentTarget) onClose() }}>
    <div className="card dialog device-link-dialog" role="dialog" aria-modal="true" aria-label={title}><h2>{title}</h2>{children}</div>
  </div>, document.body)
}

/** On the device that HAS the account: show a code the new device scans. */
export function AddDeviceModal({ onClose }: { onClose: () => void }) {
  const [code, setCode] = useState('')
  const [error, setError] = useState('')
  const [linked, setLinked] = useState<string | null>(null)
  const [scanning, setScanning] = useState(false)
  const [busy, setBusy] = useState(false)
  const done = useRef(false)
  useEffect(() => {
    let alive = true
    const stops: (() => void)[] = []
    void (async () => {
      if (HAS_TAURI) {
        const { listen } = await import('@tauri-apps/api/event')
        stops.push(await listen<{ name?: string }>('link://linked', e => { if (!alive) return; done.current = true; setLinked(e.payload?.name ?? 'Your device'); void useStore.getState().reloadFriends() }))
        stops.push(await listen<string>('link://failed', e => { if (alive) setError(typeof e.payload === 'string' ? e.payload : 'The other device couldn’t join.') }))
      }
      try { const v = await api.linkHostBegin(); if (alive) setCode(v); else void api.linkHostCancel() }
      catch (e) { if (alive) setError(`Couldn’t create a code: ${String(e)}`) }
    })()
    return () => { alive = false; stops.forEach(s => s()); if (!done.current) void api.linkHostCancel().catch(() => {}) }
  }, [])
  const scanned = async (value: string) => {
    setScanning(false); setBusy(true); setError('')
    try { const r = await linkWithCode(value); done.current = true; setLinked(r.name); void useStore.getState().reloadFriends() }
    catch (e) { setError(String(e instanceof Error ? e.message : e)) }
    finally { setBusy(false) }
  }
  if (scanning) return <QrScanner hint="Scan the code shown on your other device." onResult={v => void scanned(v)} onClose={() => setScanning(false)} />
  return <Dialog title="Add a device" onClose={onClose}>
    {linked ? <div className="account-linked"><CheckCircle2 size={40} /><p><strong>{linked}</strong> is linked. Your friends and chats are on it now and will stay in sync.</p><button className="btn btn-primary" onClick={onClose}>Done</button></div> : <>
      <p>On your phone or other computer, open DropBeam and choose <strong>Already use DropBeam?</strong> (or Settings → Devices → Scan Code), then scan this code.</p>
      {code ? <div className="link-qr"><QRCodeSVG value={code} size={220} level="M" marginSize={1} /></div> : !error && <p role="status">Creating code…</p>}
      {code && <p className="account-waiting" role="status">{busy ? 'Linking…' : 'Waiting for your other device…'}</p>}
      {error && <p role="alert" className="error-text">{error}</p>}
      <div className="device-link-actions">
        <button className="btn btn-ghost" onClick={() => setScanning(true)}><QrCode size={14} />Scan the other device instead</button>
        {code && <button className="btn btn-ghost" onClick={() => void navigator.clipboard.writeText(code).then(() => useStore.getState().toast('success', 'Code copied'))}>Copy code</button>}
        <button className="btn btn-ghost" onClick={onClose}>Cancel</button>
      </div>
    </>}
  </Dialog>
}

/** On a NEW device: scan (or paste) the code the device that has the account shows. */
export function JoinAccountModal({ onClose, onShowCode }: { onClose: () => void; onShowCode: () => void }) {
  const [scanning, setScanning] = useState(true)
  const [error, setError] = useState('')
  const [busy, setBusy] = useState(false)
  const [linked, setLinked] = useState<string | null>(null)
  const scanned = async (value: string) => {
    setScanning(false); setBusy(true); setError('')
    try { const r = await linkWithCode(value); setLinked(r.name); void useStore.getState().reloadFriends() }
    catch (e) { setError(String(e instanceof Error ? e.message : e)) }
    finally { setBusy(false) }
  }
  if (scanning) return <QrScanner hint="On your other device open Settings → Devices → Add a device, then scan its code." onResult={v => void scanned(v)} onClose={onClose} />
  return <Dialog title="Join your account" onClose={onClose}>
    {linked ? <div className="account-linked"><CheckCircle2 size={40} /><p>Linked with <strong>{linked}</strong>. Your friends and chats are syncing to this device.</p><button className="btn btn-primary" onClick={onClose}>Done</button></div> : <>
      <p role="status">{busy ? 'Linking…' : error}</p>
      {!busy && <div className="device-link-actions">
        <button className="btn btn-primary" onClick={() => { setError(''); setScanning(true) }}>Try again</button>
        <button className="btn btn-ghost" onClick={onShowCode}>Show a code instead</button>
        <button className="btn btn-ghost" onClick={onClose}>Close</button>
      </div>}
    </>}
  </Dialog>
}
