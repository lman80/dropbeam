import { groupDevices, personGroups } from '../lib/deviceIcons'
import { DeviceBadge } from '../components/DeviceBadge'
import { AddDeviceModal } from '../components/DevicesPanel'
import { useOwnDeviceLabels } from '../lib/ownDevices'
import { ConfirmDialog, SafetyMenu } from '../components/SafetyDialogs'
import { QrCodeView, ScanCodeButton, ShareCode } from '../components/CodeQr'
import { MOBILE_UI } from '../lib/platform'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { AnimatePresence } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import {
  ChevronRight,
  Plus,
  Camera,
  Check,
  Copy,
  MessageCircle,
  Pencil,
  QrCode,
  Radar,
  Send,
  Trash2,
  UserPlus,
  Users,
} from 'lucide-react'
import { api, fileSrc, HAS_TAURI, type ConnDetail, type Friend } from '../lib/api'
import { useStore } from '../store'
import { parseCode, wrongCodeMessage } from '../lib/codes'
import { MobileHeader } from '../components/MobileHeader'
import { Dialog } from '../components/Dialog'
import { createPortal } from 'react-dom'
import { ConnInfo } from '../components/ConnInspector'
import { FriendAvatar } from '../components/FriendAvatar'
import { EmptyState, IconButton, MenuButton, SectionHeader, Spinner, type MenuItem } from '../components/ui'
import { avatarColor, avatarGradient, initials } from '../lib/avatar'
import { claimPresenceChecks, friendPresence, presenceLabel } from '../lib/presence'

export function FriendsView() {
  const friends = useStore((s) => s.friends)
  const [adding, setAdding] = useState(false)
  const [linking, setLinking] = useState(false)
  const myDevice = useStore(s => s.myDevice)
  const grouped = personGroups(friends, myDevice?.account_pub)
  const { myDevices, others } = groupDevices(friends.filter(f => !grouped[f.id]), myDevice?.account_pub)
  useEffect(() => { void useStore.getState().refreshMyDevice().catch(() => {}) }, [])
  // Phone layout only (the desktop page builds its own section below).
  const devices = <section><div className="device-section-heading"><h2 className={MOBILE_UI ? 'ios-section-title' : ''}>My devices</h2><button className={MOBILE_UI ? 'ios-button' : 'btn btn-ghost'} onClick={() => setLinking(true)}>Add a device</button></div>
    {myDevices.length ? <div className={MOBILE_UI ? 'ios-list' : 'device-list'}>{myDevices.map(f => <FriendCard key={f.id} friend={f} />)}</div> : <p className={MOBILE_UI ? 'mobile-inset ios-footnote' : ''}>Link your phone or another computer</p>}
    {linking && <AddDeviceModal onClose={() => setLinking(false)} />}
  </section>

  // #34: presence must recover without a restart. Opening Friends actively
  // re-checks everyone who doesn't already read as online, instead of waiting
  // out the control beacon's 60/120/300 s backoff.
  useEffect(() => {
    const s = useStore.getState()
    for (const id of claimPresenceChecks(s.friends, s.friendSeen, s.folderStatuses)) void s.pingFriend(id)
  }, [])

  if (MOBILE_UI) return <div className="mobile-page mobile-friends">
    <MobileHeader title="Friends" actions={<button className="ios-icon" aria-label="Add friend" onClick={() => setAdding(true)}><Plus size={22} /></button>} />
    {devices}
    <h2 className="ios-section-title">You</h2><YouCard />
    <h2 className="ios-section-title">{others.length ? `Friends · ${others.length}` : 'Friends'}</h2>
    <div className="ios-list">
      <button className="ios-row mobile-location-row" onClick={() => useStore.getState().setView('locations')}><span>Browse friends’ Locations</span><ChevronRight size={18} /></button>
      {others.map(f => <FriendCard key={f.id} friend={f} />)}
    </div>
    {others.length === 0 && <div className="mobile-inset mobile-empty"><Users size={28} /><h2 className="ios-headline">No friends yet</h2><p className="ios-sub">Add a friend to send files and chat.</p><button className="ios-button ios-primary" onClick={() => setAdding(true)}>Add a friend</button></div>}
    {adding && <AddFriendModal onClose={() => setAdding(false)} />}
  </div>

  return <DesktopFriends />
}

// ── Desktop Friends page ─────────────────────────────────────────────────────
// A native list: You (profile + your code), My devices, Friends. Each person is
// one row — avatar, name, one quiet status line — with Send, Message and a "…"
// menu for everything else. Dialogs handle rename / invite / remove so rows
// never change height.

/** Round avatar with at most one overlay: a small green dot when online. */
function PersonAvatar({ friend, online, device, size = 32 }: {
  friend: Pick<Friend, 'id' | 'name' | 'avatar'> & Partial<Pick<Friend, 'accountPub' | 'deviceKind' | 'deviceOs'>>
  online?: boolean
  device?: boolean
  size?: number
}) {
  return (
    <span
      className={`fr-avatar${device ? ' is-device' : ''}`}
      style={{ width: size, height: size, fontSize: Math.round(size * 0.36), background: device ? undefined : avatarColor(friend.id) }}
      aria-hidden
    >
      <FriendAvatar friend={friend} />
      {online && <span className="fr-presence" />}
    </span>
  )
}

/** "Online", "Last seen 3h ago", "Not seen yet". */
function statusText(p: ReturnType<typeof friendPresence>) {
  return p.status === 'online' ? 'Online' : presenceLabel(p)
}

function DesktopFriends() {
  const friends = useStore((s) => s.friends)
  const myDevice = useStore((s) => s.myDevice)
  const [adding, setAdding] = useState(false)
  const [linking, setLinking] = useState(false)
  const grouped = personGroups(friends, myDevice?.account_pub)
  const { myDevices, others } = groupDevices(friends.filter((f) => !grouped[f.id]), myDevice?.account_pub)

  return (
    <div className="page friends-page">
      <div className="page-header titlebar-drag">
        <h1 className="page-title">Friends</h1>
        <div className="page-actions">
          <button className="btn btn-primary" onClick={() => setAdding(true)}>
            <UserPlus /> Add friend
          </button>
        </div>
      </div>

      <SectionHeader>You</SectionHeader>
      <YouSection />

      <SectionHeader
        count={myDevices.length}
        action={myDevices.length ? <button className="btn btn-plain btn-sm" onClick={() => setLinking(true)}>Link a device…</button> : undefined}
      >
        My devices
      </SectionHeader>
      <div className="group fr-list">
        {myDevices.map((f) => <FriendRow key={f.id} friend={f} />)}
        {myDevices.length === 0 && (
          <button className="row fr-add-row" onClick={() => setLinking(true)}>
            <span className="fr-avatar is-device" aria-hidden><Plus size={16} /></span>
            <span className="row-main">
              <span className="row-title">Link a device…</span>
              <span className="row-sub">Use DropBeam on your phone or another computer</span>
            </span>
          </button>
        )}
      </div>

      <SectionHeader count={others.length}>Friends</SectionHeader>
      {others.length === 0 ? (
        <div className="group">
          <EmptyState
            icon={<Users />}
            title="No friends yet"
            hint="Add a friend with their DropBeam code."
            action={<button className="btn btn-secondary" onClick={() => setAdding(true)}>Add friend</button>}
            style={{ padding: '32px 24px' }}
          />
        </div>
      ) : (
        <div className="group fr-list">
          {others.map((f) => <FriendRow key={f.id} friend={f} />)}
        </div>
      )}

      {linking && <AddDeviceModal onClose={() => setLinking(false)} />}
      <AnimatePresence>{adding && <AddFriendModal onClose={() => setAdding(false)} />}</AnimatePresence>
    </div>
  )
}

/** Your profile (picture + name) and your permanent DropBeam code. */
function YouSection() {
  const settings = useStore((s) => s.settings)
  const saveSettings = useStore((s) => s.saveSettings)
  const pickAvatar = useStore((s) => s.pickAvatar)
  const clearAvatar = useStore((s) => s.clearAvatar)
  const toast = useStore((s) => s.toast)
  const displayName = settings?.displayName ?? ''
  const [code, setCode] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [dialog, setDialog] = useState<null | 'name' | 'qr'>(null)

  useEffect(() => {
    let alive = true
    api
      .myInviteCode()
      .then((c) => alive && setCode(c))
      .catch(() => alive && setCode(''))
    return () => {
      alive = false
    }
  }, [])

  const copyCode = async () => {
    if (!code) return
    try {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    } catch {
      toast('error', 'Couldn’t copy the code')
    }
  }

  const me = { id: displayName || 'you', name: displayName || 'You', avatar: settings?.avatar ?? null }
  return (
    <div className="group fr-list">
      <div className="row">
        <button className="fr-you-avatar" onClick={() => void pickAvatar()} aria-label="Change picture" title="Change picture">
          <PersonAvatar friend={me} />
          <span className="fr-you-avatar-edit" aria-hidden><Camera size={14} /></span>
        </button>
        <div className="row-main">
          <div className="row-title truncate-1" title={displayName}>{displayName || 'You'}</div>
          <div className="row-sub">Your name and picture</div>
        </div>
        <div className="row-trailing">
          <MenuButton
            label="Edit profile"
            items={[
              { label: 'Edit name…', onSelect: () => setDialog('name') },
              { label: 'Change picture…', onSelect: () => void pickAvatar() },
              { label: 'Remove picture', onSelect: () => void clearAvatar(), hidden: !settings?.avatar },
            ]}
          />
        </div>
      </div>
      <div className="row">
        <span className="fr-avatar is-device" aria-hidden><QrCode size={16} /></span>
        <div className="row-main">
          <div className="row-title">Your DropBeam code</div>
          {code ? (
            <div className="row-sub fr-code truncate-1 selectable" title={code}>{code}</div>
          ) : (
            <div className="row-sub">{code === null ? 'Loading…' : 'Appears once DropBeam has connected'}</div>
          )}
        </div>
        <div className="row-trailing">
          <button className="btn btn-secondary btn-sm fr-code-btn" disabled={!code} onClick={() => setDialog('qr')}>Show QR</button>
          <button className="btn btn-secondary btn-sm fr-code-btn" disabled={!code} onClick={() => void copyCode()}>
            {copied ? <><Check /> Copied</> : 'Copy'}
          </button>
        </div>
      </div>

      <AnimatePresence>
        {dialog === 'name' && (
          <NameDialog
            key="name"
            title="Your name"
            label="Name"
            initial={displayName}
            onSave={(n) => { if (n !== displayName) void saveSettings({ displayName: n }) }}
            onClose={() => setDialog(null)}
          />
        )}
        {dialog === 'qr' && code && (
          <Dialog
            key="qr"
            title="Your DropBeam code"
            width={340}
            onClose={() => setDialog(null)}
            footer={
              <>
                <button className="btn btn-secondary" onClick={() => void copyCode()}>{copied ? 'Copied' : 'Copy code'}</button>
                <button className="btn btn-primary" autoFocus onClick={() => setDialog(null)}>Done</button>
              </>
            }
          >
            <div className="fr-qr">
              <QrCodeView value={code} size={200} hint={null} label="QR code for your DropBeam code" />
              <p>Friends scan this in DropBeam → Add friend.</p>
            </div>
          </Dialog>
        )}
      </AnimatePresence>
    </div>
  )
}

/** One-field rename dialog (friend or your own name). */
function NameDialog({ title, label, initial, onSave, onClose }: {
  title: string
  label: string
  initial: string
  onSave: (name: string) => void
  onClose: () => void
}) {
  const [name, setName] = useState(initial)
  const save = () => {
    const n = name.trim()
    if (!n) return
    onSave(n)
    onClose()
  }
  return (
    <Dialog
      title={title}
      width={360}
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-secondary" onClick={onClose}>Cancel</button>
          <button className="btn btn-primary" disabled={!name.trim()} onClick={save}>Save</button>
        </>
      }
    >
      <label className="field-label" htmlFor="fr-name-input">{label}</label>
      <input
        id="fr-name-input"
        className="input"
        value={name}
        autoFocus
        onFocus={(e) => e.currentTarget.select()}
        onChange={(e) => setName(e.target.value)}
        onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); save() } }}
      />
    </Dialog>
  )
}

/** A friend (or one of your own devices) as one list row. */
function FriendRow({ friend }: { friend: Friend }) {
  const ownLabel = useOwnDeviceLabels()[friend.id] as string | undefined
  const sendToFriend = useStore((s) => s.sendToFriend)
  const removeFriend = useStore((s) => s.removeFriend)
  const renameFriend = useStore((s) => s.renameFriend)
  const setFriendAutoAccept = useStore((s) => s.setFriendAutoAccept)
  const pingFriend = useStore((s) => s.pingFriend)
  const openChat = useStore((s) => s.openChat)
  const friendSeen = useStore((s) => s.friendSeen)
  const folderStatuses = useStore((s) => s.folderStatuses)
  const toast = useStore((s) => s.toast)
  const [busy, setBusy] = useState(false)
  const [dialog, setDialog] = useState<null | 'rename' | 'remove' | 'invite'>(null)
  const [invite, setInvite] = useState<string | null>(null)
  const [loadingInvite, setLoadingInvite] = useState(false)
  const [pinging, setPinging] = useState(false)
  const [pingedOffline, setPingedOffline] = useState(false)
  const [conn, setConn] = useState<ConnDetail | null>(null)

  const presence = friendPresence(friend.name, friendSeen, folderStatuses)
  const isOnline = presence.status === 'online'
  const channel = Object.values(folderStatuses).find(
    (s) => s.peerName?.trim().toLowerCase() === friend.name.trim().toLowerCase(),
  )?.locality
  const label = ownLabel ?? friend.name

  // Check the live path once when a friend comes online (it feeds the
  // connection-details popover); forget it when they drop. The `alive` guard
  // stops a late probe from a prior cycle clobbering current state.
  useEffect(() => {
    if (!isOnline) return
    let alive = true
    useStore
      .getState()
      .probeFriend(friend.id)
      .then((d) => { if (alive) setConn(d) })
      .catch(() => { if (alive) setConn(null) })
    return () => {
      alive = false
    }
  }, [isOnline, friend.id])

  const check = async () => {
    setPinging(true)
    setPingedOffline(false)
    try {
      const ok = await pingFriend(friend.id)
      if (ok) toast('success', `${label} is online`)
      else {
        setPingedOffline(true)
        toast('info', `${label} didn’t respond — they may be offline`)
      }
    } finally {
      setPinging(false)
    }
  }

  const send = async () => {
    setBusy(true)
    try {
      const paths = await api.pickFiles()
      if (paths.length) {
        // The store already toasted a failure; only confirm a send that started.
        if (await sendToFriend(friend.id, paths)) toast('info', `Beaming to ${label}…`)
      }
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  const showInvite = async () => {
    if (invite) return setDialog('invite')
    setLoadingInvite(true)
    try {
      setInvite(await api.friendInvite(friend.id))
      setDialog('invite')
    } catch (e) {
      toast('error', String(e))
    } finally {
      setLoadingInvite(false)
    }
  }

  // A live presence recovery clears a stale "No response" from an earlier check.
  const status = pinging ? 'Checking…' : pingedOffline && !isOnline ? 'No response' : statusText(presence)
  const check15 = <span className="menu-check" aria-hidden />
  const items: MenuItem[] = [
    { label: 'Rename…', icon: check15, onSelect: () => setDialog('rename') },
    { label: 'Show invite…', icon: check15, onSelect: () => void showInvite(), disabled: loadingInvite },
    {
      label: 'Auto-accept files',
      icon: <span className="menu-check" aria-hidden>{friend.autoAccept && <Check />}</span>,
      onSelect: () => void setFriendAutoAccept(friend.id, !friend.autoAccept),
    },
    { label: 'Check if online', icon: check15, onSelect: () => void check(), disabled: pinging },
  ]
  const removeItem: MenuItem[] = [
    { label: ownLabel ? 'Remove device…' : 'Remove friend…', icon: check15, danger: true, onSelect: () => setDialog('remove') },
  ]

  return (
    <div className="row fr-row">
      <PersonAvatar friend={friend} online={isOnline} device={!!ownLabel} />
      <div className="row-main">
        <div className="row-title truncate-1" title={label}>{label}</div>
        <div className="row-sub truncate-1">{status}</div>
      </div>
      <div className="row-trailing">
        <span className="fr-conn">{isOnline && <ConnInfo detail={conn} locality={channel} label={`Connection to ${label}`} />}</span>
        <button className="btn btn-secondary btn-sm fr-send" onClick={() => void send()} disabled={busy}>
          {busy ? <Spinner size={12} /> : <Send />} Send
        </button>
        <IconButton label={`Message ${label}`} onClick={() => openChat(friend.id)}>
          <MessageCircle />
        </IconButton>
        {ownLabel ? (
          <MenuButton label={`More options for ${label}`} items={[...items, { separator: true }, ...removeItem]} />
        ) : (
          <SafetyMenu friend={friend} before={items} after={removeItem} blankIcon={check15} />
        )}
      </div>

      <AnimatePresence>
        {dialog === 'rename' && (
          <NameDialog
            key="rename"
            title={`Rename ${friend.name}`}
            label="Name"
            initial={friend.name}
            onSave={(n) => { if (n !== friend.name) void renameFriend(friend.id, n) }}
            onClose={() => setDialog(null)}
          />
        )}
        {dialog === 'remove' && (
          <ConfirmDialog
            key="remove"
            title={`Remove ${label}?`}
            confirmLabel="Remove"
            onConfirm={() => removeFriend(friend.id)}
            onClose={() => setDialog(null)}
          >
            Your chat history stays on this device.
            {friend.endpointId && ' Add them again any time to pick up where you left off.'}
          </ConfirmDialog>
        )}
        {dialog === 'invite' && invite && (
          <Dialog
            key="invite"
            title={`Invite for ${friend.name}`}
            width={360}
            onClose={() => setDialog(null)}
            footer={<button className="btn btn-primary" autoFocus onClick={() => setDialog(null)}>Done</button>}
          >
            <ShareCode
              code={invite}
              layout="stack"
              size={180}
              hint={null}
              copyLabel="Copy invite"
              copyVariant="secondary"
              instructions={<>{friend.name} can scan this or paste it in Add friend.</>}
            />
          </Dialog>
        )}
      </AnimatePresence>
    </div>
  )
}

function AddFriendModal({ onClose }: { onClose: () => void }) {
  const acceptFriend = useStore((s) => s.acceptFriend)
  const addFriendByCode = useStore((s) => s.addFriendByCode)
  const openCode = useStore((s) => s.openCode)
  const [error, setError] = useState('')
  const [codeInput, setCodeInput] = useState('')
  const [busy, setBusy] = useState(false)

  const submit = async (value = codeInput) => {
    setError('')
    if (!value.trim()) {
      setError('Paste your friend’s code, or scan their QR code.')
      return
    }
    const parsed = parseCode(value)
    if (!parsed) {
      setError(wrongCodeMessage(['friend'], null))
      return
    }
    setBusy(true)
    try {
      if (parsed.kind === 'friendInvite') await acceptFriend(parsed.code) // legacy invite
      else if (parsed.kind === 'friend') await addFriendByCode(parsed.code)
      // Some other DropBeam code (a Quick Send, a folder invite…): do what it's for.
      else if (!(await openCode(parsed.code))) return
      onClose()
    } catch (e) {
      setError(String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      title="Add a friend"
      onClose={onClose}
      busy={busy}
      width={420}
      footer={
        <>
          <button className="btn btn-secondary" onClick={onClose} disabled={busy}>Cancel</button>
          <button className="btn btn-primary" onClick={() => void submit()} disabled={busy || !codeInput.trim()}>
            {busy && <Spinner size={13} />} Add
          </button>
        </>
      }
    >
      <label htmlFor="add-friend-code" className="field-label">Their DropBeam code</label>
      <div className="fr-code-field">
        <input
          id="add-friend-code"
          className="input"
          autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" inputMode="text"
          placeholder="dropbeam:…"
          value={codeInput}
          autoFocus
          aria-invalid={!!error}
          onChange={(e) => { setCodeInput(e.target.value); if (error) setError('') }}
          onKeyDown={(e) => { if (e.key === 'Enter') { e.preventDefault(); void submit() } }}
        />
        <ScanCodeButton
          className="btn btn-plain"
          label="Scan QR…"
          disabled={busy}
          hint="Hold your friend’s QR code up to the camera."
          title="Scan a friend’s code"
          accept={['friend', 'friendInvite']}
          onCode={(code) => { setCodeInput(code); void submit(code) }}
          onOther={(p) => { setCodeInput(p.code); void submit(p.code) }}
        />
      </div>
      {error ? (
        <p role="alert" className="form-error">{error}</p>
      ) : (
        <p className="field-hint">It’s on their Friends page, under You.</p>
      )}
    </Dialog>
  )
}

// ── Phone layout (legacy web phone UI; kept compiling) ─────────────────────

/** Reusable avatar: the user's chosen picture, or an initials monogram. */
function Avatar({
  name,
  seed,
  picture,
  size = 44,
  radius = 14,
}: {
  name: string
  seed: string
  picture?: string | null
  size?: number
  radius?: number
}) {
  const [brokenPicture, setBrokenPicture] = useState<string | null>(null)
  const showPic = !!picture && HAS_TAURI && brokenPicture !== picture
  return (
    <div
      style={{
        width: size,
        height: size,
        borderRadius: radius,
        display: 'grid',
        placeItems: 'center',
        color: 'white',
        fontWeight: 700,
        fontSize: size * 0.34,
        overflow: 'hidden',
        background: avatarGradient(seed),
      }}
    >
      {showPic ? (
        <img
          src={fileSrc(picture!)}
          alt={name}
          style={{ width: '100%', height: '100%', objectFit: 'cover' }}
          onError={() => setBrokenPicture(picture!)}
        />
      ) : (
        initials(name)
      )}
    </div>
  )
}

/** Phone layout: your profile and permanent code. */
function YouCard() {
  const settings = useStore((s) => s.settings)
  const saveSettings = useStore((s) => s.saveSettings)
  const pickAvatar = useStore((s) => s.pickAvatar)
  const clearAvatar = useStore((s) => s.clearAvatar)
  const toast = useStore((s) => s.toast)

  const displayName = settings?.displayName ?? ''
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState(displayName)
  const [code, setCode] = useState<string | null>(null)
  const [copied, setCopied] = useState(false)
  const [showQR, setShowQR] = useState(false)

  useEffect(() => {
    if (!editing) setName(displayName)
  }, [displayName, editing])

  useEffect(() => {
    let alive = true
    api
      .myInviteCode()
      .then((c) => alive && setCode(c))
      .catch(() => alive && setCode(''))
    return () => {
      alive = false
    }
  }, [])

  const saveName = () => {
    const n = name.trim()
    if (n && n !== displayName) void saveSettings({ displayName: n })
    setEditing(false)
  }

  const copyCode = async () => {
    if (!code) return
    try {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    } catch {
      toast('error', 'Could not copy')
    }
  }

  return <section className="mobile-you mobile-inset">
    <div className="mobile-profile">
      <button className="mobile-avatar-button" aria-label="Change picture" onClick={() => void pickAvatar()}><Avatar name={displayName || 'You'} seed={displayName || 'you'} picture={settings?.avatar} size={64} radius={20} /></button>
      <div className="mobile-grow">
        {editing ? <input className="input" aria-label="Your name" value={name} autoFocus onChange={e => setName(e.target.value)} onBlur={saveName} onKeyDown={e => { if (e.key === 'Enter') saveName(); if (e.key === 'Escape') { setName(displayName); setEditing(false) } }} /> : <button className="mobile-name ios-headline" onClick={() => setEditing(true)}>{displayName || 'You'}<Pencil size={16} /></button>}
        <p className="ios-footnote">The name and picture your friends see.</p>
      </div>
    </div>
    {settings?.avatar && <button className="ios-button ios-destructive" onClick={() => void clearAvatar()}>Remove picture</button>}
    <div className="mobile-equal">
      <button className="ios-button" disabled={!code} onClick={() => setShowQR(v => !v)}><QrCode size={18} />{showQR ? 'Hide QR' : 'QR code'}</button>
      <button className="ios-button" disabled={!code} onClick={copyCode}>{copied ? <Check size={18} /> : <Copy size={18} />}{copied ? 'Copied' : 'Copy code'}</button>
    </div>
    {showQR && code && <div className="mobile-qr"><QRCodeSVG value={code} size={160} level="M" /></div>}
    <p className="ios-footnote">Share this code once. Your friends stay connected across updates.</p>
  </section>

}

/** Phone layout: a friend row that opens a manage sheet. */
function FriendCard({ friend }: { friend: Friend }) {
  const sendToFriend = useStore((s) => s.sendToFriend)
  const removeFriend = useStore((s) => s.removeFriend)
  const renameFriend = useStore((s) => s.renameFriend)
  const setFriendAutoAccept = useStore((s) => s.setFriendAutoAccept)
  const pingFriend = useStore((s) => s.pingFriend)
  const openChat = useStore((s) => s.openChat)
  const friendSeen = useStore((s) => s.friendSeen)
  const folderStatuses = useStore((s) => s.folderStatuses)
  const toast = useStore((s) => s.toast)
  const [sheetOpen, setSheetOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState(false)
  const [name, setName] = useState(friend.name)
  const [invite, setInvite] = useState<string | null>(null)
  const [loadingInvite, setLoadingInvite] = useState(false)
  const [pinging, setPinging] = useState(false)

  const presence = friendPresence(friend.name, friendSeen, folderStatuses)
  const isOnline = presence.status === 'online'
  const check = async () => {
    setPinging(true)
    try {
      const ok = await pingFriend(friend.id)
      if (ok) toast('success', `${friend.name} is online`)
      else {
        toast('info', `${friend.name} didn’t respond — they may be offline`)
      }
    } finally {
      setPinging(false)
    }
  }

  const send = async () => {
    setBusy(true)
    try {
      const paths = await api.pickFiles()
      if (paths.length) {
        // The store already toasted a failure; only confirm a send that started.
        if (await sendToFriend(friend.id, paths)) toast('info', `Beaming to ${friend.name}…`)
      }
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  const showInvite = async () => {
    if (invite) {
      setInvite(null)
      return
    }
    setLoadingInvite(true)
    try {
      setInvite(await api.friendInvite(friend.id))
    } catch (e) {
      toast('error', String(e))
    } finally {
      setLoadingInvite(false)
    }
  }

  const saveName = () => {
    if (name.trim() && name.trim() !== friend.name) renameFriend(friend.id, name.trim())
  }

  return <>
    <div className="ios-row mobile-friend-row">
      <button className="mobile-friend-open" onClick={() => setSheetOpen(true)} aria-label={`Manage ${friend.name}`}>
        <span className="device-avatar"><Avatar name={friend.name} seed={friend.id} picture={friend.avatar} size={40} radius={12} /><DeviceBadge kind={friend.deviceKind} /></span>
        <span className="mobile-grow"><span className="ios-headline mobile-friend-name">{friend.name}</span><span className="ios-footnote mobile-presence"><i className={isOnline ? 'online' : ''} />{presenceLabel(presence)}</span></span>
      </button>
      <button className="ios-icon" aria-label={`Send to ${friend.name}`} onClick={send} disabled={busy}>{busy ? <Spinner size={20} /> : <Send size={20} />}</button>
    </div>
    {sheetOpen && <MobileFriendSheet title={friend.name} onClose={() => { setSheetOpen(false); setConfirmRemove(false) }}>
      <label className="ios-row"><span className="mobile-grow">Name</span><input className="input mobile-sheet-name" value={name} onChange={e => setName(e.target.value)} onBlur={saveName} /></label>
      <div className="ios-row"><span className="mobile-grow">Auto-accept files</span><button className={`toggle${friend.autoAccept ? ' on' : ''}`} role="switch" aria-checked={friend.autoAccept} aria-label="Auto-accept files" onClick={() => setFriendAutoAccept(friend.id, !friend.autoAccept)} /></div>
      <button className="ios-row" onClick={() => openChat(friend.id)}><MessageCircle size={20} />Message</button>
      <button className="ios-row" onClick={check} disabled={pinging}><Radar size={20} />{pinging ? 'Checking…' : 'Check presence'}</button>
      <button className="ios-row" onClick={showInvite} disabled={loadingInvite}><Copy size={20} />{invite ? 'Hide invite' : 'Show invite'}</button>
      {invite && <div className="mobile-inset mobile-stack"><p className="ios-footnote">Share this invite with {friend.name}.</p><button className="ios-button" onClick={() => navigator.clipboard.writeText(invite).then(() => toast('success', 'Invite copied')).catch(() => toast('error', 'Could not copy'))}>Copy invite</button></div>}
      {confirmRemove ? <div className="mobile-inset mobile-stack"><p className="ios-footnote">Remove {friend.name}? Chat history stays on this device.</p><div className="mobile-equal"><button className="ios-button" onClick={() => setConfirmRemove(false)}>Cancel</button><button className="ios-button ios-destructive" onClick={() => removeFriend(friend.id)}>Remove</button></div></div> : <button className="ios-row" onClick={() => setConfirmRemove(true)}><Trash2 size={20} /><span className="mobile-grow">Remove friend</span><span className="ios-destructive">Remove</span></button>}
    </MobileFriendSheet>}
  </>

}

/** Native modal focus trapping, Escape dismissal and focus restoration. */
function MobileFriendSheet({ title, children, onClose }: { title: string; children: ReactNode; onClose: () => void }) {
  const dialog = useRef<HTMLDialogElement>(null)
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null
    dialog.current?.showModal()
    return () => { previous?.focus() }
  }, [])
  return createPortal(<dialog ref={dialog} className="mobile-friend-sheet" aria-label={`Manage ${title}`} onCancel={onClose} onClick={e => { if (e.target === e.currentTarget) { const r = e.currentTarget.getBoundingClientRect(); if (e.clientX < r.left || e.clientX > r.right || e.clientY < r.top || e.clientY > r.bottom) onClose() } }}>
    <div className="mobile-sheet-top"><h2 className="ios-title2">{title}</h2><button className="ios-button" onClick={onClose} autoFocus>Done</button></div>
    {children}
  </dialog>, document.body)
}
