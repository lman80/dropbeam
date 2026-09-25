import { createElement, useEffect, useMemo, useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import { Plus, QrCode } from 'lucide-react'
import { api, type AccountDevice } from '../lib/api'
import { deviceIcon, deviceNoun, ownDeviceLabels } from '../lib/deviceIcons'
import { friendOnlineState } from '../lib/presence'
import { useStore } from '../store'
import { LinkFlow } from './LinkDeviceModal'
import { ConfirmDialog } from './SafetyDialogs'
import { MenuButton } from './ui'

export { linkWithCode, isDeviceCode } from './LinkDeviceModal'

function ago(ms: number) {
  const s = Math.max(0, (Date.now() - ms) / 1000)
  if (s < 60) return 'just now'
  if (s < 3600) return `${Math.floor(s / 60)} min ago`
  if (s < 86400) return `${Math.floor(s / 3600)} hr ago`
  return new Date(ms).toLocaleDateString()
}

/** Device glyph in a neutral disc (a tiny green dot when that device is online). */
function DeviceGlyph({ d, online }: { d: Pick<AccountDevice, 'device_kind' | 'device_os'>; online?: boolean }) {
  const kind = d.device_os === 'macos' && d.device_kind !== 'desktop' ? 'laptop' : d.device_kind ?? undefined
  return <span className="account-device-glyph" aria-hidden>
    <DeviceIcon kind={kind} />
    {online && <span className="account-device-dot" />}
  </span>
}
function DeviceIcon({ kind }: { kind?: string }) {
  return createElement(deviceIcon(kind), { size: 16, strokeWidth: 1.7 })
}

/** Settings → Devices on desktop: every device in this account, kept in sync peer to peer. */
export function DevicesPanel() {
  const myDevice = useStore(s => s.myDevice)
  const friends = useStore(s => s.friends)
  const friendSeen = useStore(s => s.friendSeen)
  const folderStatuses = useStore(s => s.folderStatuses)
  const [mode, setMode] = useState<null | 'show' | 'scan'>(null)
  const [confirm, setConfirm] = useState<null | { kind: 'remove'; device: AccountDevice } | { kind: 'leave' }>(null)
  const [syncing, setSyncing] = useState(false)
  const devices = useMemo(() => myDevice?.devices ?? [], [myDevice])
  const inAccount = !!myDevice?.account_pub && devices.length > 1
  const me = devices.find(d => d.this_device)
  const myNoun = deviceNoun(me?.device_kind ?? myDevice?.device_kind, me?.device_os ?? myDevice?.device_os)
  // "Your iPhone" — or "Your iPhone 2" when two would read the same.
  const labels = useMemo(() => ownDeviceLabels(devices.filter(d => !d.this_device)
    .map(d => ({ id: d.endpoint_id, name: d.name, deviceKind: d.device_kind, deviceOs: d.device_os }))), [devices])
  useEffect(() => { void useStore.getState().refreshMyDevice().catch(() => {}) }, [])
  const online = (d: AccountDevice) => { const f = friends.find(x => x.id === d.friend_id); return f ? friendOnlineState(f.name, friendSeen, folderStatuses) === true : false }
  const syncNow = async () => {
    setSyncing(true)
    try { await api.accountSyncNow(); await new Promise(r => setTimeout(r, 2500)); await useStore.getState().refreshMyDevice() } finally { setSyncing(false) }
  }
  const remove = async (d: AccountDevice) => {
    const label = labels[d.endpoint_id] ?? d.name
    try { await api.accountRemoveDevice(d.endpoint_id); useStore.getState().toast('success', `${label} was removed from your account`); await useStore.getState().reloadFriends() }
    catch (e) { useStore.getState().toast('error', String(e)) }
  }
  const leave = async () => {
    try { await api.accountLeave(); useStore.getState().toast('success', `This ${myNoun} left your account`); await useStore.getState().reloadFriends() }
    catch (e) { useStore.getState().toast('error', String(e)) }
  }
  const thisTitle = `This ${myNoun}`
  return <section className="account-devices">
    <div className="section-row">
      <h2 className="section-title">Devices</h2>
      {inAccount && <button className="btn btn-plain btn-sm account-sync" disabled={syncing} onClick={() => void syncNow()}>{syncing ? 'Syncing…' : 'Sync now'}</button>}
    </div>
    <div className="group account-device-list">
      {inAccount ? devices.map(d => {
        const title = d.this_device ? thisTitle : labels[d.endpoint_id] ?? `Your ${deviceNoun(d.device_kind, d.device_os)}`
        const on = !d.this_device && online(d)
        const sub = d.this_device ? d.name : [on ? 'Online' : 'Offline', d.last_sync_ms ? `Synced ${ago(d.last_sync_ms)}` : 'Not synced yet'].join(' · ')
        return <div key={d.endpoint_id} className="row">
          <DeviceGlyph d={d} online={on} />
          <div className="row-main">
            <div className="row-title truncate-1" title={d.name}>{title}</div>
            <div className="row-sub truncate-1">{sub}</div>
          </div>
          <div className="row-trailing">
            <MenuButton
              label={`Options for ${title}`}
              items={d.this_device
                ? [{ label: `Remove this ${myNoun} from account…`, danger: true, onSelect: () => setConfirm({ kind: 'leave' }) }]
                : [{ label: 'Remove from account…', danger: true, onSelect: () => setConfirm({ kind: 'remove', device: d }) }]}
            />
          </div>
        </div>
      }) : me && <div className="row">
        <DeviceGlyph d={me} />
        <div className="row-main">
          <div className="row-title">{thisTitle}</div>
          <div className="row-sub truncate-1">{me.name}</div>
        </div>
      </div>}
      <button className="row account-device-add" onClick={() => setMode('show')}>
        <span className="account-device-glyph" aria-hidden><Plus size={16} /></span>
        <span className="row-main">
          <span className="row-title">Link a device…</span>
          {!inAccount && <span className="row-sub">Get your friends and chats on your phone or another computer</span>}
        </span>
      </button>
      {!inAccount && <button className="row account-device-add" onClick={() => setMode('scan')}>
        <span className="account-device-glyph" aria-hidden><QrCode size={16} /></span>
        <span className="row-main"><span className="row-title">Scan another device’s code…</span></span>
      </button>}
    </div>
    {mode && <LinkFlow start={mode} title="Link a device" onClose={() => { setMode(null); void useStore.getState().refreshMyDevice().catch(() => {}) }} />}
    <AnimatePresence>
      {confirm?.kind === 'remove' && <ConfirmDialog key="remove"
        title={`Remove ${labels[confirm.device.endpoint_id] ?? confirm.device.name} from your account?`}
        confirmLabel="Remove" onConfirm={() => remove(confirm.device)} onClose={() => setConfirm(null)}>
        It stops getting your friends and chats, and is told the next time it’s online. You can link it again later.
      </ConfirmDialog>}
      {confirm?.kind === 'leave' && <ConfirmDialog key="leave"
        title={`Remove this ${myNoun} from your account?`}
        confirmLabel="Remove" onConfirm={leave} onClose={() => setConfirm(null)}>
        Your friends and chats stay on this {myNoun} but stop syncing with your other devices.
      </ConfirmDialog>}
    </AnimatePresence>
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
