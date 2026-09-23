import { groupDevices, personGroups } from '../lib/deviceIcons'
import { DeviceBadge } from '../components/DeviceBadge'
import { AddDeviceModal } from '../components/DevicesPanel'
import { useOwnDeviceLabels } from '../lib/ownDevices'
import { SafetyMenu } from '../components/SafetyDialogs'
import { ScanCodeButton, ShareCode } from '../components/CodeQr'
import { MOBILE_UI } from '../lib/platform'
import { useEffect, useRef, useState, type ReactNode } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
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
  RefreshCw,
  Send,
  Trash2,
  UserPlus,
  Users,
} from 'lucide-react'
import { api, fileSrc, HAS_TAURI, type ConnDetail, type Friend } from '../lib/api'
import { useStore } from '../store'
import { parseCode, wrongCodeMessage } from '../lib/codes'
import { ChannelBadge, EmptyState, Spinner } from '../components/bits'
import { MobileHeader } from '../components/MobileHeader'
import { Dialog } from '../components/Dialog'
import { createPortal } from 'react-dom'
import { ConnInspector } from '../components/ConnInspector'
import { avatarGradient, initials } from '../lib/avatar'
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

  return (
    <div className="page">
      <div className="page-header titlebar-drag">
        <div>
          <h1 className="page-title">Friends</h1>
          <p className="page-subtitle">Send files and chat with people by name.</p>
        </div>
        <div className="page-actions">
          <button className="btn btn-primary" onClick={() => setAdding(true)}>
            <UserPlus size={15} /> Add friend
          </button>
        </div>
      </div>

      {/* ── You ─────────────────────────────────────────────── */}
      <SectionLabel first>You</SectionLabel>
      <YouCard />

      {/* ── Your other devices (multi-device account) ───────── */}
      <div className="section-row">
        <SectionLabel>{myDevices.length ? `My devices · ${myDevices.length}` : 'My devices'}</SectionLabel>
        <button className="btn btn-quiet btn-sm" onClick={() => setLinking(true)}>
          <Plus size={14} /> Link a device
        </button>
      </div>
      {myDevices.length ? (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>{myDevices.map((f) => <FriendCard key={f.id} friend={f} />)}</div>
      ) : (
        <button className="card empty-row" onClick={() => setLinking(true)}>
          <span className="empty-row-icon"><Plus size={16} /></span>
          <span style={{ flex: 1, minWidth: 0 }}>
            <span className="empty-row-title">Link your phone or another computer</span>
            <span className="empty-row-sub">Your friends and chats follow you to every device you link.</span>
          </span>
          <ChevronRight size={16} style={{ color: 'var(--text-faint)', flexShrink: 0 }} />
        </button>
      )}
      {linking && <AddDeviceModal onClose={() => setLinking(false)} />}

      {/* ── Friends ─────────────────────────────────────────── */}
      <SectionLabel>{others.length ? `Friends · ${others.length}` : 'Friends'}</SectionLabel>
      {others.length === 0 ? (
        <div className="card" style={{ padding: '6px 0 0' }}>
          <EmptyState
            icon={<Users size={24} />}
            title="No friends yet"
            hint="Share your code with someone (or paste theirs). Add them once and you can beam files and chat by name forever — it survives app updates, so you never re-add anyone."
          />
          <div style={{ display: 'flex', justifyContent: 'center', paddingBottom: 22 }}>
            <button className="btn btn-primary" onClick={() => setAdding(true)}>
              <UserPlus size={15} /> Add a friend
            </button>
          </div>
        </div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 10 }}>
          <AnimatePresence initial={false}>
            {others.map((f) => (
              <FriendCard key={f.id} friend={f} />
            ))}
          </AnimatePresence>
        </div>
      )}

      <AnimatePresence>{adding && <AddFriendModal onClose={() => setAdding(false)} />}</AnimatePresence>
    </div>
  )
}

function SectionLabel({ children, first }: { children: ReactNode; first?: boolean }) {
  return <h2 className="section-title" style={{ margin: first ? '0 2px 9px' : '22px 2px 9px' }}>{children}</h2>
}

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

/** Your own profile: picture, editable name, and your permanent code. */
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

  if (MOBILE_UI) return <section className="mobile-you mobile-inset">
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

  return (
    <div className="card" style={{ padding: 16 }}>
      <div style={{ display: 'flex', alignItems: 'center', gap: 14 }}>
        {/* Avatar with a hover "change" affordance */}
        <button
          className="you-avatar-btn"
          title="Change picture"
          onClick={() => void pickAvatar()}
          style={{ position: 'relative', flexShrink: 0, padding: 0, border: 'none', background: 'none', cursor: 'pointer' }}
        >
          <Avatar name={displayName || 'You'} seed={displayName || 'you'} picture={settings?.avatar} size={56} radius={999} />
          <span className="you-avatar-cam">
            <Camera size={13} />
          </span>
        </button>

        <div style={{ flex: 1, minWidth: 0 }}>
          {editing ? (
            <input
              className="input"
              value={name}
              autoFocus
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') saveName()
                if (e.key === 'Escape') {
                  setName(displayName)
                  setEditing(false)
                }
              }}
              onBlur={saveName}
              style={{ fontSize: 'var(--font-md)', fontWeight: 700, padding: '5px 10px', maxWidth: 280 }}
            />
          ) : (
            <div style={{ display: 'flex', alignItems: 'center', gap: 7, minWidth: 0 }}>
              <span className="truncate-1" style={{ fontWeight: 750, fontSize: 'var(--font-lg)' }}>{displayName || 'You'}</span>
              <button
                className="icon-btn"
                title="Edit your name"
                aria-label="Edit your name"
                style={{ width: 24, height: 24, borderRadius: 6 }}
                onClick={() => {
                  setName(displayName)
                  setEditing(true)
                }}
              >
                <Pencil size={12.5} />
              </button>
            </div>
          )}
          <div style={{ fontSize: 'calc(12px * var(--ui-font-scale, 1))', color: 'var(--text-muted)', marginTop: 3 }}>
            This is the name and picture your friends see.
          </div>
          {settings?.avatar ? (
            <button
              onClick={() => void clearAvatar()}
              style={{ background: 'none', border: 'none', padding: 0, marginTop: 5, cursor: 'pointer', color: 'var(--text-faint)', fontSize: 'calc(11.5px * var(--ui-font-scale, 1))' }}
            >
              Remove picture
            </button>
          ) : null}
        </div>
      </div>

      {/* Your permanent code: QR + text + Copy */}
      <div
        style={{
          marginTop: 14,
          paddingTop: 14,
          borderTop: '1px solid var(--border)',
        }}
      >
        <div style={{ fontSize: 'calc(13px * var(--ui-font-scale, 1))', fontWeight: 650, marginBottom: 10 }}>Your DropBeam code</div>
        {code ? (
          <ShareCode
            code={code}
            size={168}
            instructions="Friends scan this QR in DropBeam (Friends → Add friend) or paste the code. Share it once — it never changes, so friends who add you stay connected across every update."
          />
        ) : (
          <div style={{ fontSize: 'calc(12px * var(--ui-font-scale, 1))', color: 'var(--text-muted)' }}>Your code appears once DropBeam has connected.</div>
        )}
      </div>
    </div>
  )
}

function FriendCard({ friend }: { friend: Friend }) {
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
  const [sheetOpen, setSheetOpen] = useState(false)
  const [busy, setBusy] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState(false)
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState(friend.name)
  const [invite, setInvite] = useState<string | null>(null)
  const [loadingInvite, setLoadingInvite] = useState(false)
  const [pinging, setPinging] = useState(false)
  const [pingedOffline, setPingedOffline] = useState(false)
  const [conn, setConn] = useState<ConnDetail | null>(null)
  const [probing, setProbing] = useState(false)

  const presence = friendPresence(friend.name, friendSeen, folderStatuses)
  const isOnline = presence.status === 'online'
  const channel = Object.values(folderStatuses).find(
    (s) => s.peerName?.trim().toLowerCase() === friend.name.trim().toLowerCase(),
  )?.locality

  const probe = async () => {
    setProbing(true)
    try {
      setConn(await useStore.getState().probeFriend(friend.id))
    } catch {
      setConn(null)
    } finally {
      setProbing(false)
    }
  }

  // Auto-check the live path once when a friend comes online; clear it when they
  // drop. The `alive` guard stops a late-resolving probe from a prior cycle (or
  // after the card unmounts/reorders) from clobbering current state.
  useEffect(() => {
    if (!isOnline) {
      setConn(null)
      return
    }
    let alive = true
    ;(async () => {
      setProbing(true)
      try {
        const d = await useStore.getState().probeFriend(friend.id)
        if (alive) setConn(d)
      } catch {
        if (alive) setConn(null)
      } finally {
        if (alive) setProbing(false)
      }
    })()
    return () => {
      alive = false
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [isOnline, friend.id])

  // A live presence recovery clears a stale "No response" from an earlier Check,
  // so the label, dot, and connection line all agree the friend is back.
  useEffect(() => {
    if (isOnline) setPingedOffline(false)
  }, [isOnline])

  const check = async () => {
    setPinging(true)
    setPingedOffline(false)
    try {
      const ok = await pingFriend(friend.id)
      if (ok) toast('success', `${friend.name} is online`)
      else {
        setPingedOffline(true)
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
    setEditing(false)
  }

  if (MOBILE_UI) return <>
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

  return (
    <motion.div
      layout
      initial={{ opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, scale: 0.97 }}
      className="card"
      style={{ padding: 14, overflow: 'hidden' }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
        <div style={{ position: 'relative', flexShrink: 0 }}>
          <Avatar name={friend.name} seed={friend.id} picture={friend.avatar} size={44} radius={999} /><DeviceBadge kind={friend.deviceKind} />
          <span
            title={presenceLabel(presence)}
            style={{
              position: 'absolute',
              right: -1,
              top: -1,
              width: 13,
              height: 13,
              borderRadius: 999,
              background: isOnline ? 'var(--green)' : 'var(--text-faint)',
              border: '2.5px solid var(--surface)',
            }}
          />
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          {editing ? (
            <input
              className="input"
              value={name}
              autoFocus
              onChange={(e) => setName(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === 'Enter') saveName()
                if (e.key === 'Escape') {
                  setName(friend.name)
                  setEditing(false)
                }
              }}
              onBlur={saveName}
              style={{ fontSize: 'var(--font-md)', fontWeight: 650, padding: '5px 10px', maxWidth: 260 }}
            />
          ) : (
            <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
              <span className="truncate-1" style={{ fontWeight: 700, fontSize: 'var(--font-md)' }} title={ownLabel ?? friend.name}>{ownLabel ?? friend.name}</span>
              {ownLabel && <span className="truncate-1" style={{ color: 'var(--text-faint)', fontSize: 'var(--font-xs)' }}>{friend.name}</span>}
              <button
                className="icon-btn"
                title="Rename"
                aria-label={`Rename ${friend.name}`}
                style={{ width: 22, height: 22, borderRadius: 6 }}
                onClick={() => {
                  setName(friend.name)
                  setEditing(true)
                }}
              >
                <Pencil size={12} />
              </button>
            </div>
          )}
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 7,
              fontSize: 'calc(12px * var(--ui-font-scale, 1))',
              marginTop: 2,
              flexWrap: 'wrap',
              color: isOnline ? 'var(--green)' : 'var(--text-faint)',
            }}
          >
            <span>{pingedOffline ? 'No response' : presenceLabel(presence)}</span>
            {isOnline && conn ? (
              <>
                <span style={{ color: 'var(--border-strong)' }}>·</span>
                <ConnInspector detail={conn} compact />
                <button
                  onClick={probe}
                  disabled={probing}
                  title="Re-check how you're connected"
                  style={{
                    background: 'none',
                    border: 'none',
                    padding: 0,
                    cursor: 'pointer',
                    color: 'var(--text-faint)',
                    display: 'inline-flex',
                    alignItems: 'center',
                  }}
                >
                  <RefreshCw size={11} className={probing ? 'spin' : undefined} />
                </button>
              </>
            ) : isOnline && probing ? (
              <>
                <span style={{ color: 'var(--border-strong)' }}>·</span>
                <span style={{ display: 'inline-flex', alignItems: 'center', gap: 4, color: 'var(--text-faint)' }}>
                  <Spinner size={11} /> checking link…
                </span>
              </>
            ) : (
              channel && channel !== 'unknown' && <ChannelBadge locality={channel} size={11} />
            )}
            <span style={{ color: 'var(--border-strong)' }}>·</span>
            <button
              onClick={check}
              disabled={pinging}
              style={{
                background: 'none',
                border: 'none',
                padding: 0,
                cursor: 'pointer',
                color: 'var(--accent)',
                fontSize: 'calc(12px * var(--ui-font-scale, 1))',
                fontWeight: 600,
                display: 'inline-flex',
                alignItems: 'center',
                gap: 4,
              }}
            >
              {pinging ? <Spinner size={11} /> : <Radar size={12} />}
              {pinging ? 'Checking…' : 'Check'}
            </button>
          </div>
        </div>
        <button
          className="icon-btn"
          title={`Message ${friend.name}`}
          aria-label={`Message ${friend.name}`}
          onClick={() => openChat(friend.id)}
          style={{ width: 36, height: 36 }}
        >
          <MessageCircle size={18} />
        </button>
        <button className="btn btn-primary" onClick={send} disabled={busy} style={{ flexShrink: 0 }}>
          {busy ? <Spinner size={14} /> : <Send size={15} />} Send
        </button>
      </div>

      {/* One compact management row: auto-accept + invite + remove */}
      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 10,
          marginTop: 12,
          paddingTop: 12,
          borderTop: '1px solid var(--border)',
        }}
      >
        <button
          className={`toggle${friend.autoAccept ? ' on' : ''}`}
          role="switch"
          aria-checked={friend.autoAccept}
          aria-label={`Auto-accept files from ${friend.name}`}
          title={friend.autoAccept ? 'Files save automatically' : 'You approve each file'}
          onClick={() => setFriendAutoAccept(friend.id, !friend.autoAccept)}
        />
        <span style={{ fontSize: 'var(--font-sm)', color: 'var(--text-muted)', flex: 1, minWidth: 0 }}>
          {friend.autoAccept ? 'Auto-accept files' : 'Approve files first'}
        </span>
        <button className="btn btn-ghost btn-sm" onClick={showInvite} disabled={loadingInvite}>
          {loadingInvite ? <Spinner size={13} /> : <Copy size={13} />} {invite ? 'Hide invite' : 'Invite'}
        </button>
        {!ownLabel && !confirmRemove && <SafetyMenu friend={friend} />}
        {confirmRemove ? (
          <>
            <button className="btn btn-ghost btn-sm" onClick={() => setConfirmRemove(false)}>
              Cancel
            </button>
            <button className="btn btn-danger btn-sm" onClick={() => removeFriend(friend.id)}>
              <Trash2 size={13} /> Remove
            </button>
          </>
        ) : (
          <button className="icon-btn icon-btn-danger" title="Remove friend" aria-label={`Remove ${friend.name}`} onClick={() => setConfirmRemove(true)}>
            <Trash2 size={15} />
          </button>
        )}
      </div>

      {confirmRemove && (
        <p role="status" style={{ fontSize: 'calc(12.5px * var(--ui-font-scale, 1))', color: 'var(--text-muted)', margin: '8px 0 0' }}>
          Remove {friend.name}? Your chat history will be kept on this device.
          {friend.endpointId && ' Re-add the same device to restore the conversation.'}
        </p>
      )}

      {invite && <InvitePanel invite={invite} friendName={friend.name} onClose={() => setInvite(null)} />}
    </motion.div>
  )
}

/** Inline invite reveal (re-show an existing friend's invite). */
function InvitePanel({
  invite,
  friendName,
  onClose,
}: {
  invite: string
  friendName: string
  onClose: () => void
}) {
  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: 'auto' }}
      style={{ overflow: 'hidden', marginTop: 12 }}
    >
      <div style={{ borderTop: '1px solid var(--border)', paddingTop: 14 }}>
        <ShareCode
          code={invite}
          size={168}
          copyLabel="Copy invite"
          instructions={<>Send this to {friendName}. They open DropBeam → Friends → <b>Add friend</b> and scan this QR code or paste the invite.</>}
          footer={<button className="btn btn-ghost" onClick={onClose}>Hide</button>}
        />
      </div>
    </motion.div>
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
      setError("Paste your friend's code, or scan their QR code.")
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
      subtitle="Scan their QR code or paste their DropBeam code."
      icon={<UserPlus size={18} />}
      onClose={onClose}
      busy={busy}
      footer={
        <button className="btn btn-primary btn-block" onClick={() => void submit()} disabled={busy}>
          {busy ? <Spinner size={15} /> : <UserPlus size={15} />} Add friend
        </button>
      }
    >
      <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10, marginBottom: 6 }}>
        <label htmlFor="add-friend-code" className="field-label" style={{ margin: 0 }}>
          Your friend's code
        </label>
        <ScanCodeButton
          small
          disabled={busy}
          hint="Hold your friend’s QR code (Friends → You) up to your camera."
          title="Scan a friend’s code"
          accept={['friend', 'friendInvite']}
          onCode={(code) => { setCodeInput(code); void submit(code) }}
          onOther={(p) => { setCodeInput(p.code); void submit(p.code) }}
        />
      </div>
      <textarea
        id="add-friend-code"
        className="input"
        style={{ minHeight: 72, fontFamily: 'var(--font-mono)', fontSize: 'var(--font-sm)', resize: 'none', wordBreak: 'break-all' }}
        autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" inputMode="text"
        placeholder="Paste their dropbeam:… code, or scan their QR"
        value={codeInput}
        autoFocus
        onChange={(e) => setCodeInput(e.target.value)}
        onKeyDown={(e) => { if (e.key === 'Enter' && !e.shiftKey) { e.preventDefault(); void submit() } }}
      />
      {error && <p role="alert" className="form-error">{error}</p>}
      <p className="dialog-text" style={{ margin: '12px 0 0' }}>
        Ask your friend for their code (Friends → <b>You</b>) — scan the QR on their screen or paste the code. Their
        name fills in automatically and you’ll both be connected — no retyping names, no re-adding after updates.
      </p>
    </Dialog>
  )
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
