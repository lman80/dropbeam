import { createElement, useEffect, useMemo, useState } from 'react'
import { relativeTime } from '../lib/dates'
import { AnimatePresence } from 'framer-motion'
import { Plus, QrCode } from 'lucide-react'
import { api, type AccountDevice } from '../lib/api'
import { deviceIcon, deviceNoun, ownDeviceLabels } from '../lib/deviceIcons'
import { friendOnlineState } from '../lib/presence'
import { useStore } from '../store'
import { LinkFlow } from './LinkDeviceModal'
import { ConfirmDialog } from './SafetyDialogs'
import { MenuButton } from './ui'

// eslint-disable-next-line react-refresh/only-export-components -- re-exported helpers for existing importers
export { linkWithCode, isDeviceCode } from './LinkDeviceModal'

const ago = (ms: number) => relativeTime(ms).replace(/^Just now$/, 'just now')

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
  const [confirm, setConfirm] = useState<null | { kind: 'remove'; device: AccountDevice } | { kind: 'approve'; device: AccountDevice } | { kind: 'leave' }>(null)
  const [syncing, setSyncing] = useState(false)
  const devices = useMemo(() => myDevice?.devices ?? [], [myDevice])
  const inAccount = !!myDevice?.account_pub && devices.length > 1
  const me = devices.find(d => d.this_device)
  const myNoun = deviceNoun(me?.device_kind ?? myDevice?.device_kind, me?.device_os ?? myDevice?.device_os)
  // "Your iPhone" — or "Your iPhone 15" / "Your iPhone 12" when two would read the same.
  const labels = useMemo(() => ownDeviceLabels(devices.filter(d => !d.this_device)
    .map(d => ({ id: d.endpoint_id, name: d.name, deviceKind: d.device_kind, deviceOs: d.device_os, deviceModel: d.device_model }))), [devices])
  useEffect(() => { void useStore.getState().refreshMyDevice().catch(() => {}) }, [])
  const online = (d: AccountDevice) => { const f = friends.find(x => x.id === d.friend_id); return f ? friendOnlineState(f, friendSeen, folderStatuses) === true : false }
  const syncNow = async () => {
    setSyncing(true)
    try { await api.accountSyncNow(); await new Promise(r => setTimeout(r, 2500)); await useStore.getState().refreshMyDevice() } finally { setSyncing(false) }
  }
  const remove = async (d: AccountDevice) => {
    const label = labels[d.endpoint_id] ?? d.name
    try { await api.accountRemoveDevice(d.endpoint_id); useStore.getState().toast('success', `${label} was removed from your devices`); await useStore.getState().reloadFriends() }
    catch (e) { useStore.getState().toast('error', String(e)) }
  }
  const approve = async (d: AccountDevice) => {
    try { await api.accountApproveDevice(d.endpoint_id); useStore.getState().toast('success', `${d.name} is now one of your devices`); await useStore.getState().reloadFriends(); await useStore.getState().refreshMyDevice() }
    catch (e) { useStore.getState().toast('error', String(e)) }
  }
  const leave = async () => {
    try { await api.accountLeave(); useStore.getState().toast('success', `This ${myNoun} is no longer linked to your other devices`); await useStore.getState().reloadFriends() }
    catch (e) { useStore.getState().toast('error', String(e)) }
  }
  const thisTitle = `This ${myNoun}`
  return <section className="account-devices">
    <div className="section-row">
      <h2 className="section-title">My devices</h2>
      {inAccount && <button className="btn btn-plain btn-sm account-sync" disabled={syncing} onClick={() => void syncNow()}>{syncing ? 'Syncing…' : 'Sync Now'}</button>}
    </div>
    <p className="account-devices-note intro">Link your phone and computers so they’re all you: the same friends, chats, name and photo on each.</p>
    <div className="group account-device-list">
      {inAccount ? devices.map(d => {
        const title = d.this_device ? thisTitle : labels[d.endpoint_id] ?? `Your ${deviceNoun(d.device_kind, d.device_os)}`
        const on = !d.this_device && online(d)
        const sub = d.this_device ? d.name : d.needs_approval ? `${d.name} · Says it’s yours, but none of your devices added it`
          : [on ? 'Online' : 'Offline', d.last_sync_ms ? `Synced ${ago(d.last_sync_ms)}` : 'Not synced yet'].join(' · ')
        return <div key={d.endpoint_id} className="row">
          <DeviceGlyph d={d} online={on} />
          <div className="row-main">
            <div className="row-title truncate-1" title={d.name}>{title}</div>
            <div className="row-sub truncate-1">{sub}</div>
          </div>
          <div className="row-trailing">
            {d.needs_approval && <>
              <button className="btn btn-plain btn-sm" onClick={() => setConfirm({ kind: 'approve', device: d })}>It’s Mine…</button>
              <button className="btn btn-secondary btn-sm" onClick={() => setConfirm({ kind: 'remove', device: d })}>Remove…</button>
            </>}
            <MenuButton
              label={`Options for ${title}`}
              items={d.this_device
                ? [{ label: `Remove This ${myNoun} from My Devices…`, danger: true, onSelect: () => setConfirm({ kind: 'leave' }) }]
                : [{ label: 'Remove Device…', danger: true, onSelect: () => setConfirm({ kind: 'remove', device: d }) }]}
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
          <span className="row-title">Link a Device…</span>
          {!inAccount && <span className="row-sub">Get your friends and chats on your phone or another computer</span>}
        </span>
      </button>
      {!inAccount && <button className="row account-device-add" onClick={() => setMode('scan')}>
        <span className="account-device-glyph" aria-hidden><QrCode size={16} /></span>
        <span className="row-main"><span className="row-title">Scan the Other Device…</span></span>
      </button>}
    </div>
    <p className="account-devices-note">{inAccount
      ? 'Lost a phone or computer? Remove it here so it gets no new messages. Messages already on it stay there, so also lock or erase it (on an iPhone, with Find My).'
      : 'Your friends and chats are only on your devices — there’s no online backup. Link a second device so losing one doesn’t lose them.'}</p>
    {mode && <LinkFlow start={mode} title="Link a Device" onClose={() => { setMode(null); void useStore.getState().refreshMyDevice().catch(() => {}) }} />}
    <AnimatePresence>
      {confirm?.kind === 'remove' && <ConfirmDialog key="remove"
        title={`Remove ${labels[confirm.device.endpoint_id] ?? confirm.device.name}?`}
        confirmLabel="Remove" onConfirm={() => remove(confirm.device)} onClose={() => setConfirm(null)}>
        It stops getting your new messages and friends, and is told the next time it’s online. What’s already on it stays there. You can link it again later.
      </ConfirmDialog>}
      {confirm?.kind === 'approve' && <ConfirmDialog key="approve"
        title={`Is “${confirm.device.name}” yours?`}
        confirmLabel="Yes, It’s Mine" danger={false} onConfirm={() => approve(confirm.device)} onClose={() => setConfirm(null)}>
        Only say yes if you set up DropBeam on this device yourself. Saying yes gives it all your friends and chats. Not sure? Choose Remove instead — you can link it again later.
      </ConfirmDialog>}
      {confirm?.kind === 'leave' && <ConfirmDialog key="leave"
        title={`Remove this ${myNoun} from your devices?`}
        confirmLabel="Remove" onConfirm={leave} onClose={() => setConfirm(null)}>
        Your friends and chats stay on this {myNoun}, but it stops syncing with your other devices.
      </ConfirmDialog>}
    </AnimatePresence>
  </section>
}

/** Show this device's code; the other device scans it (or this one scans theirs). */
export function AddDeviceModal({ onClose }: { onClose: () => void }) {
  return <LinkFlow start="show" title="Link a Device" onClose={onClose} />
}

/** A new device joining: straight to the camera, with "show my code" as the way back. */
export function JoinAccountModal({ onClose }: { onClose: () => void; onShowCode?: () => void }) {
  return <LinkFlow start="scan" title="Link to your other device" onClose={onClose} />
}
