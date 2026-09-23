import { ScanCodeButton, ShareCode } from './CodeQr'
import { parseCode, wrongCodeMessage } from '../lib/codes'
import { useState } from 'react'
import { ArrowLeftRight, ArrowRight, Check, FolderOpen, FolderSync } from 'lucide-react'
import { Dialog } from './Dialog'
import { folderName as baseName } from '../lib/syncedFolders'
import { api } from '../lib/api'
import { useStore } from '../store'
import { friendOnlineState } from '../lib/presence'
import { Spinner } from './bits'

export function PairingModal({
  mode,
  onClose,
  initialInvite = '',
}: {
  mode: 'create' | 'accept'
  onClose: () => void
  /** Accept mode: an invite already scanned/pasted elsewhere (prefilled). */
  initialInvite?: string
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
  const [inviteInput, setInviteInput] = useState(initialInvite)
  const [createdInvite, setCreatedInvite] = useState<string | null>(null)
  const [busy, setBusy] = useState(false)

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
      // Otherwise reveal the code to share manually. The invite delivery is a
      // ~1-minute best-effort dial, so be HONEST about offline friends instead of
      // toasting success for an invite that may never arrive.
      if (invitedNames.length) {
        const { friendSeen, folderStatuses } = useStore.getState()
        const offline = invitees
          .map((fid) => friends.find((f) => f.id === fid))
          .filter((f) => f && invitedNames.includes(f.name))
          .filter((f) => friendOnlineState(f!.name, friendSeen, folderStatuses) !== true)
          .map((f) => f!.name)
        const online = invitedNames.filter((n) => !offline.includes(n))
        if (online.length) toast('success', `Invited ${online.join(', ')} to “${folderName}”.`)
        if (offline.length) {
          toast(
            'info',
            `${offline.join(', ')} ${offline.length === 1 ? 'is' : 'are'} offline — if the invite doesn't arrive, share the folder's code from its card.`,
          )
        }
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
    const parsed = parseCode(inviteInput)
    if (parsed?.kind !== 'folderInvite') { toast('error', wrongCodeMessage(['folderInvite'], parsed)); return }
    if (!folder) {
      toast('error', 'Choose a folder for the shared files.')
      return
    }
    setBusy(true)
    try {
      await api.acceptPair(parsed.code, folder)
      await reloadPairs()
      toast('success', 'Paired! Files will now sync automatically.')
      onClose()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  const folderName = folder ? baseName(folder) || folder : ''

  return (
    <Dialog
      title={createdInvite ? 'Share this invite' : mode === 'create' ? 'New shared folder' : 'Accept an invite'}
      subtitle={createdInvite ? undefined : mode === 'create' ? 'Keep a folder in sync with friends, peer-to-peer.' : 'Join a folder someone shared with you.'}
      icon={<FolderSync size={18} />}
      width={470}
      onClose={onClose}
      busy={busy}
      footer={
            createdInvite ? (
              <button className="btn btn-ghost btn-block" onClick={onClose}>Done</button>
            ) : (
              <button
                className="btn btn-primary btn-block"
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
            )
      }
    >
          {/* CREATE — invite reveal */}
          {createdInvite ? (
            <div>
              <p className="dialog-text">
                Send this invite to the other person. In their DropBeam, they choose{' '}
                <b>Accept invite</b>, scan this QR code (or paste the invite) and pick a folder. After that, anything dropped in{' '}
                <b>{folderName}</b> beams over automatically.
              </p>
              <ShareCode code={createdInvite} layout="stack" copyLabel="Copy invite" />
            </div>
          ) : (
            <div>
              {/* folder picker */}
              <label className="field-label">
                {mode === 'create' ? 'Folder to share' : 'Folder to receive into'}
              </label>
              <button
                className="btn btn-ghost btn-block"
                style={{ justifyContent: 'flex-start', padding: '10px 12px' }}
                onClick={pickFolder}
                title={folder || undefined}
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
                  <div style={{ display: 'flex', alignItems: 'center', justifyContent: 'space-between', gap: 10 }}>
                    <label htmlFor="folder-invite-code" className="field-label" style={{ margin: 0 }}>
                      Invite code
                    </label>
                    <ScanCodeButton
                      small
                      hint="Hold the folder invite QR code up to your camera."
                      title="Scan a folder invite"
                      accept={['folderInvite']}
                      onCode={(code) => setInviteInput(code)}
                    />
                  </div>
                  <textarea
                    id="folder-invite-code"
                    className="input"
                    style={{ marginTop: 6, minHeight: 70, fontFamily: 'var(--font-mono)', fontSize: 'var(--font-sm)', resize: 'none', wordBreak: 'break-all' }}
                    autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" inputMode="text"
                    placeholder="Paste the dropbeam1:… invite, or scan its QR"
                    value={inviteInput}
                    onChange={(e) => setInviteInput(e.target.value)}
                  />
                </div>
              )}

              {mode === 'create' && (
                <div style={{ marginTop: 16 }}>
                  <label className="field-label">
                    Their name <span className="optional">(optional)</span>
                  </label>
                  <input
                    className="input"
                    placeholder="e.g. Alex"
                    value={peerName}
                    onChange={(e) => setPeerName(e.target.value)}
                  />
                  <div className="field-hint">
                    Add a name and you'll be linked as friends automatically — then you can beam files
                    to each other without sharing a code again.
                  </div>
                </div>
              )}

              {mode === 'create' && (
                <div style={{ marginTop: 16 }}>
                  <label className="field-label">
                    Who can do what
                  </label>
                  <div role="radiogroup" aria-label="Who can do what" style={{ display: 'flex', flexDirection: 'column', gap: 8 }}>
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
                  <label className="field-label">
                    Invite friends{' '}
                    <span className="optional">(optional)</span>
                  </label>
                  <div style={{ display: 'flex', flexWrap: 'wrap', gap: 8 }}>
                    {friends.map((f) => {
                      const on = invitees.includes(f.id)
                      return (
                        <button
                          key={f.id}
                          type="button"
                          className={`pick-chip${on ? ' on' : ''}`}
                          aria-pressed={on}
                          onClick={() =>
                            setInvitees((prev) =>
                              prev.includes(f.id)
                                ? prev.filter((x) => x !== f.id)
                                : [...prev, f.id],
                            )
                          }
                        >
                          {on && <Check size={13} />}
                          <span className="truncate-1" style={{ maxWidth: 180 }}>{f.name}</span>
                        </button>
                      )
                    })}
                  </div>
                  <div className="field-hint">
                    They’ll get a prompt to accept and choose where to save the folder. Or skip
                    this and share the invite code with anyone.
                  </div>
                </div>
              )}
            </div>
          )}
    </Dialog>
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
    <button type="button" role="radio" aria-checked={active} className={`option-card${active ? ' on' : ''}`} onClick={onClick}>
      <div className="option-card-title">
        {icon} {title}
      </div>
      <div className="option-card-desc">{desc}</div>
    </button>
  )
}
