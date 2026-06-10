import { useEffect, useState, type ReactNode } from 'react'
import { convertFileSrc } from '@tauri-apps/api/core'
import { AnimatePresence, motion } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import {
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
  X,
} from 'lucide-react'
import { api, HAS_TAURI, type Friend } from '../lib/api'
import { useStore } from '../store'
import { ChannelBadge, EmptyState, Spinner } from '../components/bits'
import { avatarGradient, initials } from '../lib/avatar'
import { friendPresence, presenceLabel } from '../lib/presence'

export function FriendsView() {
  const friends = useStore((s) => s.friends)
  const [adding, setAdding] = useState(false)

  return (
    <div style={{ maxWidth: 640, margin: '0 auto', padding: '8px 28px 40px' }}>
      <div
        className="titlebar-drag"
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBottom: 16,
          gap: 12,
        }}
      >
        <h1 style={{ fontSize: 20, fontWeight: 750, margin: 0 }}>Friends</h1>
        <button className="btn btn-primary" onClick={() => setAdding(true)}>
          <UserPlus size={15} /> Add friend
        </button>
      </div>

      {/* ── You ─────────────────────────────────────────────── */}
      <SectionLabel>You</SectionLabel>
      <YouCard />

      {/* ── Friends ─────────────────────────────────────────── */}
      <SectionLabel>{friends.length ? `Friends · ${friends.length}` : 'Friends'}</SectionLabel>
      {friends.length === 0 ? (
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
            {friends.map((f) => (
              <FriendCard key={f.id} friend={f} />
            ))}
          </AnimatePresence>
        </div>
      )}

      {adding && <AddFriendModal onClose={() => setAdding(false)} />}
    </div>
  )
}

function SectionLabel({ children }: { children: ReactNode }) {
  return (
    <div
      style={{
        fontSize: 11.5,
        fontWeight: 700,
        letterSpacing: 0.5,
        textTransform: 'uppercase',
        color: 'var(--text-faint)',
        margin: '18px 2px 9px',
      }}
    >
      {children}
    </div>
  )
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
  const showPic = !!picture && HAS_TAURI
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
          src={convertFileSrc(picture!)}
          alt={name}
          style={{ width: '100%', height: '100%', objectFit: 'cover' }}
          onError={(e) => ((e.currentTarget as HTMLImageElement).style.display = 'none')}
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
          <Avatar name={displayName || 'You'} seed={displayName || 'you'} picture={settings?.avatar} size={56} radius={18} />
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
              style={{ fontSize: 15, fontWeight: 700, padding: '6px 10px', maxWidth: 260 }}
            />
          ) : (
            <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
              <span style={{ fontWeight: 750, fontSize: 17 }}>{displayName || 'You'}</span>
              <button
                className="icon-btn"
                title="Edit your name"
                style={{ width: 24, height: 24 }}
                onClick={() => {
                  setName(displayName)
                  setEditing(true)
                }}
              >
                <Pencil size={12.5} />
              </button>
            </div>
          )}
          <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 3 }}>
            This is the name and picture your friends see.
          </div>
          {settings?.avatar ? (
            <button
              onClick={() => void clearAvatar()}
              style={{ background: 'none', border: 'none', padding: 0, marginTop: 5, cursor: 'pointer', color: 'var(--text-faint)', fontSize: 11.5 }}
            >
              Remove picture
            </button>
          ) : null}
        </div>
      </div>

      {/* Your permanent code */}
      <div
        style={{
          marginTop: 14,
          paddingTop: 14,
          borderTop: '1px solid var(--border)',
        }}
      >
        <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 8 }}>
          <div style={{ fontSize: 13, fontWeight: 650 }}>Your DropBeam code</div>
          <div style={{ display: 'flex', gap: 6 }}>
            <button className="btn btn-ghost" onClick={() => setShowQR((v) => !v)} disabled={!code}>
              <QrCode size={14} /> {showQR ? 'Hide QR' : 'QR'}
            </button>
            <button className={`btn ${copied ? 'btn-ghost' : 'btn-primary'}`} onClick={copyCode} disabled={!code}>
              {copied ? <Check size={14} /> : <Copy size={14} />} {copied ? 'Copied' : 'Copy code'}
            </button>
          </div>
        </div>
        <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 6, lineHeight: 1.5 }}>
          Share this once. It never changes — friends who add you stay connected across every update.
        </div>
        <AnimatePresence>
          {showQR && code && (
            <motion.div
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: 'auto' }}
              exit={{ opacity: 0, height: 0 }}
              style={{ overflow: 'hidden' }}
            >
              <div style={{ display: 'flex', justifyContent: 'center', paddingTop: 14 }}>
                <div style={{ background: '#fff', padding: 12, borderRadius: 14, border: '1px solid var(--border)' }}>
                  <QRCodeSVG value={code} size={132} level="M" fgColor="#15161d" bgColor="#fff" />
                </div>
              </div>
            </motion.div>
          )}
        </AnimatePresence>
      </div>
    </div>
  )
}

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
  const [busy, setBusy] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState(false)
  const [editing, setEditing] = useState(false)
  const [name, setName] = useState(friend.name)
  const [invite, setInvite] = useState<string | null>(null)
  const [loadingInvite, setLoadingInvite] = useState(false)
  const [pinging, setPinging] = useState(false)
  const [pingedOffline, setPingedOffline] = useState(false)

  const presence = friendPresence(friend.name, friendSeen, folderStatuses)
  const isOnline = presence.status === 'online'
  const channel = Object.values(folderStatuses).find(
    (s) => s.peerName?.trim().toLowerCase() === friend.name.trim().toLowerCase(),
  )?.locality

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
        await sendToFriend(friend.id, paths)
        toast('info', `Beaming to ${friend.name}…`)
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
          <Avatar name={friend.name} seed={friend.id} picture={friend.avatar} size={44} radius={14} />
          <span
            title={presenceLabel(presence)}
            style={{
              position: 'absolute',
              right: -2,
              bottom: -2,
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
              style={{ fontSize: 14.5, fontWeight: 650, padding: '6px 10px', maxWidth: 240 }}
            />
          ) : (
            <div style={{ display: 'flex', alignItems: 'center', gap: 7 }}>
              <span style={{ fontWeight: 700, fontSize: 15 }}>{friend.name}</span>
              <button
                className="icon-btn"
                title="Rename"
                style={{ width: 22, height: 22 }}
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
              fontSize: 12,
              marginTop: 2,
              flexWrap: 'wrap',
              color: isOnline ? 'var(--green)' : 'var(--text-faint)',
            }}
          >
            <span>{pingedOffline ? 'No response' : presenceLabel(presence)}</span>
            {channel && channel !== 'unknown' && <ChannelBadge locality={channel} size={11} />}
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
                fontSize: 12,
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
          onClick={() => openChat(friend.id)}
          style={{ flexShrink: 0, width: 38, height: 38 }}
        >
          <MessageCircle size={18} />
        </button>
        <button className="btn btn-primary" onClick={send} disabled={busy}>
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
          title={friend.autoAccept ? 'Files save automatically' : 'You approve each file'}
          onClick={() => setFriendAutoAccept(friend.id, !friend.autoAccept)}
        />
        <span style={{ fontSize: 12.5, color: 'var(--text-muted)', flex: 1 }}>
          {friend.autoAccept ? 'Auto-accept files' : 'Approve files first'}
        </span>
        <button className="btn btn-ghost" onClick={showInvite} disabled={loadingInvite}>
          {loadingInvite ? <Spinner size={13} /> : <Copy size={13} />} {invite ? 'Hide invite' : 'Invite'}
        </button>
        {confirmRemove ? (
          <>
            <button className="btn btn-ghost" onClick={() => setConfirmRemove(false)}>
              Cancel
            </button>
            <button className="btn btn-danger" onClick={() => removeFriend(friend.id)}>
              <Trash2 size={14} /> Remove
            </button>
          </>
        ) : (
          <button className="icon-btn" title="Remove friend" onClick={() => setConfirmRemove(true)}>
            <Trash2 size={14} />
          </button>
        )}
      </div>

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
  const toast = useStore((s) => s.toast)
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(invite)
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    } catch {
      toast('error', 'Could not copy')
    }
  }
  return (
    <motion.div
      initial={{ opacity: 0, height: 0 }}
      animate={{ opacity: 1, height: 'auto' }}
      style={{ overflow: 'hidden', marginTop: 12 }}
    >
      <div
        style={{
          borderTop: '1px solid var(--border)',
          paddingTop: 14,
          display: 'flex',
          gap: 14,
          alignItems: 'center',
          flexWrap: 'wrap',
        }}
      >
        <div style={{ background: '#fff', padding: 10, borderRadius: 12, border: '1px solid var(--border)' }}>
          <QRCodeSVG value={invite} size={92} level="M" fgColor="#15161d" bgColor="#fff" />
        </div>
        <div style={{ flex: 1, minWidth: 200 }}>
          <div style={{ fontSize: 12.5, color: 'var(--text-muted)', marginBottom: 6, lineHeight: 1.45 }}>
            Send this to {friendName}. They open DropBeam → Friends → <b>Add friend</b>.
          </div>
          <div style={{ display: 'flex', gap: 8 }}>
            <button className={`btn ${copied ? 'btn-ghost' : 'btn-primary'}`} onClick={copy} style={{ flex: 1 }}>
              {copied ? <Check size={14} /> : <Copy size={14} />}
              {copied ? 'Copied' : 'Copy invite'}
            </button>
            <button className="btn btn-ghost" onClick={onClose}>
              Hide
            </button>
          </div>
        </div>
      </div>
    </motion.div>
  )
}

function AddFriendModal({ onClose }: { onClose: () => void }) {
  const acceptFriend = useStore((s) => s.acceptFriend)
  const addFriendByCode = useStore((s) => s.addFriendByCode)
  const toast = useStore((s) => s.toast)
  const [codeInput, setCodeInput] = useState('')
  const [busy, setBusy] = useState(false)

  const submit = async () => {
    const code = codeInput.trim()
    if (!code) {
      toast('error', "Paste your friend's code first.")
      return
    }
    setBusy(true)
    try {
      if (code.startsWith('dropbeamf1:')) await acceptFriend(code) // legacy invite
      else if (code.startsWith('dropbeam:')) await addFriendByCode(code)
      else {
        toast('error', "That doesn't look like a DropBeam code.")
        return
      }
      onClose()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  return (
    <AnimatePresence>
      <motion.div
        initial={{ opacity: 0 }}
        animate={{ opacity: 1 }}
        exit={{ opacity: 0 }}
        onClick={onClose}
        style={{
          position: 'fixed',
          inset: 0,
          background: 'rgba(8, 9, 14, 0.5)',
          backdropFilter: 'blur(4px)',
          display: 'grid',
          placeItems: 'center',
          zIndex: 200,
          padding: 20,
        }}
      >
        <motion.div
          initial={{ opacity: 0, scale: 0.96, y: 8 }}
          animate={{ opacity: 1, scale: 1, y: 0 }}
          exit={{ opacity: 0, scale: 0.97 }}
          transition={{ type: 'spring', stiffness: 320, damping: 28 }}
          onClick={(e) => e.stopPropagation()}
          className="card"
          style={{ width: 440, maxWidth: '100%', padding: 22, borderRadius: 20 }}
        >
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 14 }}>
            <div style={{ fontSize: 17, fontWeight: 750 }}>Add a friend</div>
            <button className="icon-btn" onClick={onClose}>
              <X size={17} />
            </button>
          </div>
          <label style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--text-muted)' }}>
            Your friend's code
          </label>
          <textarea
            className="input"
            style={{ marginTop: 6, minHeight: 70, fontFamily: 'var(--font-mono)', fontSize: 12, resize: 'none' }}
            placeholder="Paste their dropbeam:… code here"
            value={codeInput}
            autoFocus
            onChange={(e) => setCodeInput(e.target.value)}
          />
          <p style={{ fontSize: 12.5, color: 'var(--text-muted)', lineHeight: 1.5, marginTop: 10 }}>
            Ask your friend for their code (Friends → <b>You</b> → Copy code) and paste it here. Their
            name fills in automatically and you’ll both be connected — no retyping names, no re-adding
            after updates.
          </p>
          <button
            className="btn btn-primary"
            style={{ width: '100%', marginTop: 12 }}
            onClick={submit}
            disabled={busy}
          >
            {busy ? <Spinner size={15} /> : <UserPlus size={15} />} Add friend
          </button>
        </motion.div>
      </motion.div>
    </AnimatePresence>
  )
}
