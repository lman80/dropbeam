import { useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import { ArrowLeftRight, ArrowRight, Check, Copy, FolderOpen, FolderSync, X } from 'lucide-react'
import { api } from '../lib/api'
import { useStore } from '../store'
import { Spinner } from './bits'

export function PairingModal({
  mode,
  onClose,
}: {
  mode: 'create' | 'accept'
  onClose: () => void
}) {
  const reloadPairs = useStore((s) => s.reloadPairs)
  const reloadFriends = useStore((s) => s.reloadFriends)
  const toast = useStore((s) => s.toast)
  const friends = useStore((s) => s.friends)
  const [folder, setFolder] = useState('')
  const [syncMode, setSyncMode] = useState<'mirror' | 'twoway' | 'oneway'>('twoway')
  const [peerName, setPeerName] = useState('')
  // Existing friends the user picked to invite straight into this folder (no code).
  const [invitees, setInvitees] = useState<string[]>([])
  const [inviteInput, setInviteInput] = useState('')
  const [createdInvite, setCreatedInvite] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)
  const [copied, setCopied] = useState(false)

  const pickFolder = async () => {
    const d = await api.pickDirectory()
    if (d) setFolder(d)
  }

  const doCreate = async () => {
    if (!folder) {
      toast('error', 'Choose a folder to share first.')
      return
    }
    setBusy(true)
    try {
      const res = await api.createPair(
        folder,
        syncMode !== 'oneway',
        peerName.trim() || undefined,
        syncMode === 'mirror',
      )
      // Invite the friends the user picked — each gets a prompt on their side.
      let invitedNames: string[] = []
      if (invitees.length) {
        // Reuse create_pair's own pending invite for the FIRST friend (so its link
        // isn't left dangling as a stuck "waiting to join"); mint fresh group
        // invites for the rest.
        const results = await Promise.allSettled(
          invitees.map((fid, i) =>
            api.inviteFriendToFolder(res.pair.id, fid, i === 0 ? res.invite : null),
          ),
        )
        invitedNames = invitees
          .filter((_, i) => results[i].status === 'fulfilled')
          .map((fid) => friends.find((f) => f.id === fid)?.name || 'friend')
        const failed = results.filter((r) => r.status === 'rejected').length
        if (failed) toast('error', `Couldn't invite ${failed} friend(s) — share the code instead.`)
      }
      reloadPairs()
      if (peerName.trim()) reloadFriends()
      // If we invited friends directly, that's the whole flow — confirm + close.
      // Otherwise reveal the code to share manually.
      if (invitedNames.length) {
        toast('success', `Invited ${invitedNames.join(', ')} to “${folderName}”.`)
        onClose()
      } else {
        setCreatedInvite(res.invite)
      }
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  const doAccept = async () => {
    if (!inviteInput.trim()) {
      toast('error', 'Paste the invite code from the other person.')
      return
    }
    if (!folder) {
      toast('error', 'Choose a folder for the shared files.')
      return
    }
    setBusy(true)
    try {
      await api.acceptPair(inviteInput.trim(), folder)
      await reloadPairs()
      toast('success', 'Paired! Files will now sync automatically.')
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

  const folderName = folder ? folder.split('/').pop() || folder : ''

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
              {createdInvite
                ? 'Share this invite'
                : mode === 'create'
                  ? 'New Shared Folder'
                  : 'Accept an invite'}
            </div>
            <button className="icon-btn" onClick={onClose}>
              <X size={17} />
            </button>
          </div>

          {/* CREATE — invite reveal */}
          {createdInvite ? (
            <div>
              <p style={{ fontSize: 13.5, color: 'var(--text-muted)', lineHeight: 1.5, marginTop: 0 }}>
                Send this invite to the other person. In their DropBeam, they choose{' '}
                <b>Accept invite</b> and pick a folder. After that, anything dropped in{' '}
                <b>{folderName}</b> beams over automatically.
              </p>
              <div
                style={{
                  display: 'flex',
                  gap: 16,
                  alignItems: 'center',
                  marginTop: 8,
                  flexWrap: 'wrap',
                }}
              >
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
          ) : (
            <div>
              {/* folder picker */}
              <label style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--text-muted)' }}>
                {mode === 'create' ? 'Folder to share' : 'Folder to receive into'}
              </label>
              <button
                className="btn btn-ghost"
                style={{ width: '100%', justifyContent: 'flex-start', marginTop: 6, padding: '11px 12px' }}
                onClick={pickFolder}
              >
                <FolderOpen size={16} />
                <span
                  style={{
                    overflow: 'hidden',
                    textOverflow: 'ellipsis',
                    whiteSpace: 'nowrap',
                    color: folder ? 'var(--text)' : 'var(--text-faint)',
                  }}
                >
                  {folder || 'Choose a folder…'}
                </span>
              </button>

              {mode === 'accept' && (
                <div style={{ marginTop: 16 }}>
                  <label style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--text-muted)' }}>
                    Invite code
                  </label>
                  <textarea
                    className="input"
                    style={{ marginTop: 6, minHeight: 70, fontFamily: 'var(--font-mono)', fontSize: 12, resize: 'none' }}
                    placeholder="Paste the dropbeam1:… invite here"
                    value={inviteInput}
                    onChange={(e) => setInviteInput(e.target.value)}
                  />
                </div>
              )}

              {mode === 'create' && (
                <div style={{ marginTop: 16 }}>
                  <label style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--text-muted)' }}>
                    Their name <span style={{ color: 'var(--text-faint)', fontWeight: 500 }}>(optional)</span>
                  </label>
                  <input
                    className="input"
                    style={{ marginTop: 6 }}
                    placeholder="e.g. Alex"
                    value={peerName}
                    onChange={(e) => setPeerName(e.target.value)}
                  />
                  <div style={{ fontSize: 11.5, color: 'var(--text-faint)', marginTop: 6, lineHeight: 1.45 }}>
                    Add a name and you'll be linked as friends automatically — then you can beam files
                    to each other without sharing a code again.
                  </div>
                </div>
              )}

              {mode === 'create' && (
                <div style={{ marginTop: 16 }}>
                  <label style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--text-muted)' }}>
                    Who can do what
                  </label>
                  <div style={{ display: 'flex', flexDirection: 'column', gap: 8, marginTop: 8 }}>
                    <DirOption
                      active={syncMode === 'mirror'}
                      onClick={() => setSyncMode('mirror')}
                      icon={<FolderSync size={16} />}
                      title="Full access · total sync"
                      desc="Everyone can add, edit, and delete — both folders stay identical (a shared source of truth). Deleted files are kept in history so nothing is lost."
                    />
                    <DirOption
                      active={syncMode === 'twoway'}
                      onClick={() => setSyncMode('twoway')}
                      icon={<ArrowLeftRight size={16} />}
                      title="Full access · no deletes"
                      desc="Both sides can add and change files and they sync both ways, but deletes stay local (a safer shared drop)."
                    />
                    <DirOption
                      active={syncMode === 'oneway'}
                      onClick={() => setSyncMode('oneway')}
                      icon={<ArrowRight size={16} />}
                      title="View only (read-only for them)"
                      desc="You’re the owner: your files flow to everyone you invite, but their changes never come back to you. They can view and download — not change your folder."
                    />
                  </div>
                </div>
              )}

              {mode === 'create' && friends.length > 0 && (
                <div style={{ marginTop: 16 }}>
                  <label style={{ fontSize: 12.5, fontWeight: 600, color: 'var(--text-muted)' }}>
                    Invite friends{' '}
                    <span style={{ color: 'var(--text-faint)', fontWeight: 500 }}>(optional)</span>
                  </label>
                  <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8, marginTop: 8 }}>
                    {friends.map((f) => {
                      const on = invitees.includes(f.id)
                      return (
                        <button
                          key={f.id}
                          type="button"
                          onClick={() =>
                            setInvitees((prev) =>
                              prev.includes(f.id)
                                ? prev.filter((x) => x !== f.id)
                                : [...prev, f.id],
                            )
                          }
                          style={{
                            display: 'flex',
                            alignItems: 'center',
                            gap: 6,
                            padding: '6px 11px',
                            borderRadius: 999,
                            fontSize: 12.5,
                            fontWeight: 600,
                            cursor: 'default',
                            transition: 'all 0.14s',
                            border: `1.5px solid ${on ? 'var(--accent)' : 'var(--border)'}`,
                            background: on ? 'var(--accent-soft)' : 'var(--surface-2)',
                            color: on ? 'var(--accent)' : 'var(--text)',
                          }}
                        >
                          {on && <Check size={13} />}
                          {f.name}
                        </button>
                      )
                    })}
                  </div>
                  <div
                    style={{
                      fontSize: 11.5,
                      color: 'var(--text-faint)',
                      marginTop: 6,
                      lineHeight: 1.45,
                    }}
                  >
                    They’ll get a prompt to accept and choose where to save the folder. Or skip
                    this and share the invite code with anyone.
                  </div>
                </div>
              )}

              <button
                className="btn btn-primary"
                style={{ width: '100%', marginTop: 20 }}
                onClick={mode === 'create' ? doCreate : doAccept}
                disabled={busy}
              >
                {busy ? <Spinner size={15} /> : null}
                {mode === 'create'
                  ? invitees.length > 0
                    ? `Create & invite ${invitees.length}`
                    : 'Create & get invite'
                  : 'Pair folder'}
              </button>
            </div>
          )}
        </motion.div>
      </motion.div>
    </AnimatePresence>
  )
}

function DirOption({
  active,
  onClick,
  icon,
  title,
  desc,
}: {
  active: boolean
  onClick: () => void
  icon: React.ReactNode
  title: string
  desc: string
}) {
  return (
    <button
      onClick={onClick}
      style={{
        flex: 1,
        textAlign: 'left',
        padding: '11px 13px',
        borderRadius: 13,
        border: `1.5px solid ${active ? 'var(--accent)' : 'var(--border)'}`,
        background: active ? 'var(--accent-soft)' : 'var(--surface-2)',
        cursor: 'default',
        transition: 'all 0.14s',
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 7, color: active ? 'var(--accent)' : 'var(--text)', fontWeight: 650, fontSize: 13.5 }}>
        {icon} {title}
      </div>
      <div style={{ fontSize: 11.5, color: 'var(--text-muted)', marginTop: 3 }}>{desc}</div>
    </button>
  )
}
