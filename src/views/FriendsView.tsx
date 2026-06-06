import { useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import {
  Check,
  Copy,
  Pencil,
  Radar,
  Send,
  Trash2,
  UserPlus,
  Users,
  X,
} from 'lucide-react'
import { api, type Friend } from '../lib/api'
import { useStore } from '../store'
import { EmptyState, Spinner } from '../components/bits'
import { avatarGradient, initials } from '../lib/avatar'
import { friendOnlineState } from '../lib/presence'

export function FriendsView() {
  const friends = useStore((s) => s.friends)
  const [modal, setModal] = useState<'add' | 'accept' | null>(null)

  return (
    <div style={{ maxWidth: 660, margin: '0 auto', padding: '8px 28px 36px' }}>
      <div
        className="titlebar-drag"
        style={{
          display: 'flex',
          alignItems: 'center',
          justifyContent: 'space-between',
          marginBottom: 18,
          gap: 12,
        }}
      >
        <h1 style={{ fontSize: 20, fontWeight: 750, margin: 0 }}>Friends</h1>
        <div style={{ display: 'flex', gap: 8 }}>
          <button className="btn btn-ghost" onClick={() => setModal('accept')}>
            <Check size={15} /> Accept invite
          </button>
          <button className="btn btn-primary" onClick={() => setModal('add')}>
            <UserPlus size={15} /> Add friend
          </button>
        </div>
      </div>

      {friends.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={<Users size={24} />}
            title="No friends yet"
            hint="Add a friend once and you can beam files straight to them by name — no codes, no QR. Anything they send you lands automatically in your downloads."
          />
          <div style={{ display: 'flex', gap: 10, justifyContent: 'center', paddingBottom: 24 }}>
            <button className="btn btn-ghost" onClick={() => setModal('accept')}>
              Accept an invite
            </button>
            <button className="btn btn-primary" onClick={() => setModal('add')}>
              <UserPlus size={15} /> Add a friend
            </button>
          </div>
        </div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
          <AnimatePresence initial={false}>
            {friends.map((f) => (
              <FriendCard key={f.id} friend={f} />
            ))}
          </AnimatePresence>
        </div>
      )}

      {modal && <FriendModal mode={modal} onClose={() => setModal(null)} />}
    </div>
  )
}

function FriendCard({ friend }: { friend: Friend }) {
  const sendToFriend = useStore((s) => s.sendToFriend)
  const removeFriend = useStore((s) => s.removeFriend)
  const renameFriend = useStore((s) => s.renameFriend)
  const setFriendAutoAccept = useStore((s) => s.setFriendAutoAccept)
  const pingFriend = useStore((s) => s.pingFriend)
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

  const online = friendOnlineState(friend.name, friendSeen, folderStatuses)

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
      style={{ padding: 16, overflow: 'hidden' }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 12 }}>
        <div style={{ position: 'relative', flexShrink: 0 }}>
          <div
            style={{
              width: 44,
              height: 44,
              borderRadius: 14,
              display: 'grid',
              placeItems: 'center',
              color: 'white',
              fontWeight: 700,
              fontSize: 15,
              background: avatarGradient(friend.id),
            }}
          >
            {initials(friend.name)}
          </div>
          <span
            title={online ? 'Online' : 'Status unknown'}
            style={{
              position: 'absolute',
              right: -2,
              bottom: -2,
              width: 13,
              height: 13,
              borderRadius: 999,
              background: online ? 'var(--green)' : 'var(--text-faint)',
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
                style={{ width: 24, height: 24 }}
                onClick={() => {
                  setName(friend.name)
                  setEditing(true)
                }}
              >
                <Pencil size={12.5} />
              </button>
            </div>
          )}
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 7,
              fontSize: 12,
              marginTop: 3,
              color: online ? 'var(--green)' : 'var(--text-faint)',
            }}
          >
            <span>{online ? 'Online' : pingedOffline ? 'No response' : 'Status unknown'}</span>
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
        <button className="btn btn-primary" onClick={send} disabled={busy}>
          {busy ? <Spinner size={14} /> : <Send size={15} />} Send files
        </button>
      </div>

      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 12,
          marginTop: 12,
          paddingTop: 12,
          borderTop: '1px solid var(--border)',
        }}
      >
        <div style={{ flex: 1 }}>
          <div style={{ fontSize: 13.5, fontWeight: 600 }}>Auto-accept files</div>
          <div style={{ fontSize: 11.5, color: 'var(--text-muted)', marginTop: 2, lineHeight: 1.4 }}>
            {friend.autoAccept
              ? 'Files arrive and save automatically'
              : "You'll approve each incoming file first"}
          </div>
        </div>
        <button
          className={`toggle${friend.autoAccept ? ' on' : ''}`}
          title={friend.autoAccept ? 'Auto-accept on' : 'Manual approval'}
          onClick={() => setFriendAutoAccept(friend.id, !friend.autoAccept)}
        />
      </div>

      <div
        style={{
          display: 'flex',
          gap: 8,
          justifyContent: 'flex-end',
          marginTop: 12,
        }}
      >
        <button className="btn btn-ghost" onClick={showInvite} disabled={loadingInvite}>
          {loadingInvite ? <Spinner size={13} /> : <Copy size={13} />} Show invite
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
          <button className="btn btn-danger" onClick={() => setConfirmRemove(true)}>
            <Trash2 size={14} /> Remove
          </button>
        )}
      </div>

      {invite && (
        <InvitePanel
          invite={invite}
          friendName={friend.name}
          onClose={() => setInvite(null)}
        />
      )}
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
            Send this to {friendName}. They open DropBeam → Friends → <b>Accept invite</b>.
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

function FriendModal({ mode, onClose }: { mode: 'add' | 'accept'; onClose: () => void }) {
  const createFriend = useStore((s) => s.createFriend)
  const acceptFriend = useStore((s) => s.acceptFriend)
  const toast = useStore((s) => s.toast)
  const [name, setName] = useState('')
  const [inviteInput, setInviteInput] = useState('')
  const [createdInvite, setCreatedInvite] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [copied, setCopied] = useState(false)

  const doAdd = async () => {
    if (!name.trim()) {
      toast('error', "Give your friend a name first.")
      return
    }
    setBusy(true)
    try {
      const invite = await createFriend(name.trim())
      setCreatedInvite(invite)
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  const doAccept = async () => {
    if (!inviteInput.trim()) {
      toast('error', 'Paste the invite from your friend.')
      return
    }
    setBusy(true)
    try {
      await acceptFriend(inviteInput.trim())
      onClose()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  const copyInvite = async () => {
    if (!createdInvite) return
    try {
      await navigator.clipboard.writeText(createdInvite)
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    } catch {
      toast('error', 'Could not copy to clipboard')
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
          style={{ width: 460, maxWidth: '100%', padding: 22, borderRadius: 20 }}
        >
          <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', marginBottom: 16 }}>
            <div style={{ fontSize: 17, fontWeight: 750 }}>
              {createdInvite ? 'Share this invite' : mode === 'add' ? 'Add a friend' : 'Accept an invite'}
            </div>
            <button className="icon-btn" onClick={onClose}>
              <X size={17} />
            </button>
          </div>

          {createdInvite ? (
            <div>
              <p style={{ fontSize: 13.5, color: 'var(--text-muted)', lineHeight: 1.5, marginTop: 0 }}>
                Send this invite to <b>{name.trim()}</b>. They open DropBeam → <b>Friends</b> →{' '}
                <b>Accept invite</b> and paste it. After that you can both beam files to each other by
                name — no codes.
              </p>
              <div style={{ display: 'flex', gap: 16, alignItems: 'center', marginTop: 8, flexWrap: 'wrap' }}>
                <div style={{ background: '#fff', padding: 12, borderRadius: 14, border: '1px solid var(--border)' }}>
                  <QRCodeSVG value={createdInvite} size={120} level="M" fgColor="#15161d" bgColor="#fff" />
                </div>
                <div style={{ flex: 1, minWidth: 200 }}>
                  <div
                    className="selectable"
                    style={{
                      fontFamily: 'var(--font-mono)',
                      fontSize: 11.5,
                      background: 'var(--surface-2)',
                      border: '1px solid var(--border)',
                      borderRadius: 11,
                      padding: '10px 12px',
                      wordBreak: 'break-all',
                      maxHeight: 96,
                      overflowY: 'auto',
                      color: 'var(--text-muted)',
                      lineHeight: 1.5,
                    }}
                  >
                    {createdInvite}
                  </div>
                  <button
                    className={`btn ${copied ? 'btn-ghost' : 'btn-primary'}`}
                    style={{ width: '100%', marginTop: 10 }}
                    onClick={copyInvite}
                  >
                    {copied ? <Check size={15} /> : <Copy size={15} />}
                    {copied ? 'Copied to clipboard' : 'Copy invite'}
                  </button>
                </div>
              </div>
              <button className="btn btn-ghost" style={{ width: '100%', marginTop: 16 }} onClick={onClose}>
                Done
              </button>
            </div>
          ) : mode === 'add' ? (
            <div>
              <label style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--text-muted)' }}>
                Their name
              </label>
              <input
                className="input"
                style={{ marginTop: 6 }}
                placeholder="e.g. Alex"
                value={name}
                autoFocus
                onChange={(e) => setName(e.target.value)}
                onKeyDown={(e) => e.key === 'Enter' && doAdd()}
              />
              <p style={{ fontSize: 12.5, color: 'var(--text-muted)', lineHeight: 1.5, marginTop: 10 }}>
                We'll create a one-time invite to send them. Once they accept, you can beam files
                back and forth just by tapping their name.
              </p>
              <button
                className="btn btn-primary"
                style={{ width: '100%', marginTop: 12 }}
                onClick={doAdd}
                disabled={busy}
              >
                {busy ? <Spinner size={15} /> : <UserPlus size={15} />} Create invite
              </button>
            </div>
          ) : (
            <div>
              <label style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--text-muted)' }}>
                Invite from your friend
              </label>
              <textarea
                className="input"
                style={{ marginTop: 6, minHeight: 70, fontFamily: 'var(--font-mono)', fontSize: 12, resize: 'none' }}
                placeholder="Paste the dropbeamf1:… invite here"
                value={inviteInput}
                autoFocus
                onChange={(e) => setInviteInput(e.target.value)}
              />
              <button
                className="btn btn-primary"
                style={{ width: '100%', marginTop: 16 }}
                onClick={doAccept}
                disabled={busy}
              >
                {busy ? <Spinner size={15} /> : <Check size={15} />} Add friend
              </button>
            </div>
          )}
        </motion.div>
      </motion.div>
    </AnimatePresence>
  )
}
