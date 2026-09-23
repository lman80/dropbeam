import { useEffect, useMemo, useState } from 'react'
import { MoreHorizontal, Plus, QrCode, RefreshCw } from 'lucide-react'
import { api, type AccountDevice } from '../lib/api'
import { deviceIcon, deviceNoun, ownDeviceLabels } from '../lib/deviceIcons'
import { friendOnlineState } from '../lib/presence'
import { useStore } from '../store'
import { LinkFlow } from './LinkDeviceModal'

export { linkWithCode, isDeviceCode } from './LinkDeviceModal'

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
  const displayName = useStore(s => s.settings?.displayName ?? '')
  const [mode, setMode] = useState<null | 'show' | 'scan'>(null)
  const [menu, setMenu] = useState<string | null>(null)
  const [syncing, setSyncing] = useState(false)
  const devices = useMemo(() => myDevice?.devices ?? [], [myDevice])
  const inAccount = !!myDevice?.account_pub && devices.length > 1
  const me = devices.find(d => d.this_device)
  const myNoun = deviceNoun(me?.device_kind ?? myDevice?.device_kind, me?.device_os ?? myDevice?.device_os)
  // "Your iPhone" — or "Your iPhone 2" when two would read the same.
  const labels = useMemo(() => ownDeviceLabels(devices.filter(d => !d.this_device)
    .map(d => ({ id: d.endpoint_id, name: d.name, deviceKind: d.device_kind, deviceOs: d.device_os }))), [devices])
  useEffect(() => { void useStore.getState().refreshMyDevice().catch(() => {}) }, [])
  useEffect(() => {
    if (!menu) return
    const close = (e: KeyboardEvent) => { if (e.key === 'Escape') setMenu(null) }
    window.addEventListener('keydown', close)
    return () => window.removeEventListener('keydown', close)
  }, [menu])
  const online = (d: AccountDevice) => { const f = friends.find(x => x.id === d.friend_id); return f ? friendOnlineState(f.name, friendSeen, folderStatuses) === true : false }
  const syncNow = async () => {
    setSyncing(true)
    try { await api.accountSyncNow(); await new Promise(r => setTimeout(r, 2500)); await useStore.getState().refreshMyDevice() } finally { setSyncing(false) }
  }
  const remove = async (d: AccountDevice) => {
    setMenu(null)
    const label = labels[d.endpoint_id] ?? d.name
    if (!window.confirm(`Remove ${label} from your account?\n\nIt stops getting your friends and chats, and it’s told the next time it’s online. You can link it again later.`)) return
    try { await api.accountRemoveDevice(d.endpoint_id); useStore.getState().toast('success', `${label} was removed from your account`); await useStore.getState().reloadFriends() }
    catch (e) { useStore.getState().toast('error', String(e)) }
  }
  const leave = async () => {
    if (!window.confirm(`Remove this ${myNoun} from your account?\n\nYour friends and chats stay on this ${myNoun}, but stop syncing with your other devices. Devices that are offline are told the next time they see this one.`)) return
    try { await api.accountLeave(); useStore.getState().toast('success', `This ${myNoun} left your account`); await useStore.getState().reloadFriends() }
    catch (e) { useStore.getState().toast('error', String(e)) }
  }
  return <section className="card account-devices" style={{ padding: 18, marginBottom: 16 }}>
    <div className="account-devices-head">
      <div>
        <h2>Devices</h2>
        <p className="account-devices-sub">Your friends, chats, name and photo sync directly between your devices — end-to-end encrypted, no cloud.</p>
      </div>
      {inAccount && <button className="btn btn-ghost" disabled={syncing} onClick={() => void syncNow()}><RefreshCw size={14} className={syncing ? 'spin' : ''} />{syncing ? 'Syncing…' : 'Sync now'}</button>}
    </div>
    {inAccount ? <>
      <ul className="account-device-list">
        {devices.map(d => {
          const title = d.this_device ? `This ${deviceNoun(d.device_kind, d.device_os)}` : labels[d.endpoint_id] ?? `Your ${deviceNoun(d.device_kind, d.device_os)}`
          const sub = d.this_device ? d.name : [d.name, online(d) ? 'Online' : 'Offline', d.last_sync_ms ? `Synced ${ago(d.last_sync_ms)}` : 'Not synced yet'].filter(Boolean).join(' · ')
          return <li key={d.endpoint_id} className="account-device">
            <DeviceGlyph d={d} />
            <span className="account-device-text"><strong>{title}</strong><span>{sub}</span></span>
            {!d.this_device && <span className="account-device-menu">
              <button className="icon-btn" aria-label={`Options for ${title}`} aria-expanded={menu === d.endpoint_id} onClick={() => setMenu(menu === d.endpoint_id ? null : d.endpoint_id)}><MoreHorizontal size={16} /></button>
              {menu === d.endpoint_id && <div className="account-device-popover" role="menu"><button role="menuitem" className="danger" onClick={() => void remove(d)}>Remove from account</button></div>}
            </span>}
          </li>
        })}
      </ul>
      {displayName && <p className="account-devices-profile">Friends see you as <strong>{displayName}</strong> on every device. Change your name or photo on any of them and the others follow.</p>}
      <div className="device-link-actions">
        <button className="btn btn-primary" onClick={() => setMode('show')}><Plus size={14} />Link a device</button>
        <button className="btn btn-ghost danger-text" onClick={() => void leave()}>Remove this {myNoun} from account</button>
      </div>
    </> : <div className="account-devices-empty">
      <p><strong>Use DropBeam on your phone or another computer?</strong> Link them and each one gets your friends and chats right away — then everything stays in sync.</p>
      <div className="device-link-actions">
        <button className="btn btn-primary" onClick={() => setMode('show')}><Plus size={14} />Link a device</button>
        <button className="btn btn-ghost" onClick={() => setMode('scan')}><QrCode size={14} />Scan the other device’s code</button>
      </div>
    </div>}
    {mode && <LinkFlow start={mode} title="Link a device" onClose={() => { setMode(null); void useStore.getState().refreshMyDevice().catch(() => {}) }} />}
  </section>
}

/** Show this device's code; the other device scans it (or this one scans theirs). */
export function AddDeviceModal({ onClose }: { onClose: () => void }) {
  return <LinkFlow start="show" title="Link a device" onClose={onClose} />
}

/** A new device joining: straight to the camera, with "show my code" as the way back. */
export function JoinAccountModal({ onClose }: { onClose: () => void; onShowCode?: () => void }) {
  return <LinkFlow start="scan" title="Link to your other device" onClose={onClose} />
}
