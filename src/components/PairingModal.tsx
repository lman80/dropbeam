import { ScanCodeButton, ShareCode } from './CodeQr'
import { parseCode, wrongCodeMessage } from '../lib/codes'
import { useState } from 'react'
import { Check, FolderOpen } from 'lucide-react'
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
            `${offline.join(', ')} ${offline.length === 1 ? 'is' : 'are'} offline. If the invite doesn’t arrive, share the folder’s invite code.`,
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
      toast('success', 'Joined the folder. Files will sync automatically.')
      onClose()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(false)
    }
  }

  const folderName = folder ? baseName(folder) || folder : ''

  if (createdInvite) {
    return (
      <Dialog
        title={`Invite to “${folderName}”`}
        width={420}
        onClose={onClose}
        footer={<button className="btn btn-secondary" onClick={onClose}>Done</button>}
      >
        <p className="dialog-text">In DropBeam, they choose Accept invite and scan or paste this.</p>
        <ShareCode code={createdInvite} layout="stack" copyLabel="Copy invite" />
      </Dialog>
    )
  }

  return (
    <Dialog
      title={mode === 'create' ? 'New shared folder' : 'Accept a folder invite'}
      width={460}
      className="folder-dialog"
      onClose={onClose}
      busy={busy}
      footer={
        <>
          <button className="btn btn-secondary" onClick={onClose} disabled={busy}>Cancel</button>
          <button
            className="btn btn-primary"
            onClick={mode === 'create' ? doCreate : doAccept}
            disabled={busy}
          >
            {busy ? <Spinner size={13} /> : null}
            {mode === 'create'
              ? invitees.length > 0
                ? `Create & invite ${invitees.length}`
                : 'Create'
              : 'Accept'}
          </button>
        </>
      }
    >
      {mode === 'accept' && (
        <div className="folder-field">
          <div className="folder-field-head">
            <label htmlFor="folder-invite-code" className="field-label">Invite</label>
            <ScanCodeButton
              small
              className="btn btn-plain btn-sm"
              hint="Hold the folder invite QR code up to your camera."
              title="Scan a folder invite"
              accept={['folderInvite']}
              onCode={(code) => setInviteInput(code)}
            />
          </div>
          <textarea
            id="folder-invite-code"
            className="input folder-code-input"
            autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" inputMode="text"
            placeholder="Paste the invite"
            value={inviteInput}
            onChange={(e) => setInviteInput(e.target.value)}
          />
        </div>
      )}

      <div className="folder-field">
        <label className="field-label">{mode === 'create' ? 'Folder to share' : 'Save into'}</label>
        <button className="btn btn-secondary folder-picker" onClick={pickFolder} title={folder || undefined}>
          <FolderOpen />
          <span className={folder ? undefined : 'placeholder'}>{folder ? folderName : 'Choose a folder…'}</span>
        </button>
      </div>

      {mode === 'create' && (
        <div className="folder-field">
          <label className="field-label" id="folder-mode-label">Access</label>
          <div className="group folder-mode-group" role="radiogroup" aria-labelledby="folder-mode-label">
            <ModeOption
              active={syncMode === 'mirror'}
              onClick={() => setSyncMode('mirror')}
              title="Total sync"
              desc="Everyone adds, edits and deletes. Both folders stay identical."
            />
            <ModeOption
              active={syncMode === 'twoway'}
              onClick={() => setSyncMode('twoway')}
              title="Two-way"
              desc="Everyone adds and edits. Deletes stay on each side."
            />
            <ModeOption
              active={syncMode === 'oneway'}
              onClick={() => setSyncMode('oneway')}
              title="View only"
              desc="Only you make changes. Others get a read-only copy."
            />
          </div>
        </div>
      )}

      {mode === 'create' && friends.length > 0 && (
        <div className="folder-field">
          <label className="field-label">
            Invite friends <span className="optional">(optional)</span>
          </label>
          <div className="folder-chips">
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
        </div>
      )}

      {mode === 'create' && (
        <div className="folder-field">
          <label className="field-label" htmlFor="folder-peer-name">
            {friends.length > 0 ? 'Or someone new' : 'Their name'} <span className="optional">(optional)</span>
          </label>
          <input
            id="folder-peer-name"
            className="input"
            placeholder="Name"
            value={peerName}
            onChange={(e) => setPeerName(e.target.value)}
          />
          <div className="field-hint">They’re added as a friend when they join.</div>
        </div>
      )}
    </Dialog>
  )
}

function ModeOption({
  active,
  onClick,
  title,
  desc,
}: {
  active: boolean
  onClick: () => void
  title: string
  desc: string
}) {
  return (
    <button type="button" role="radio" aria-checked={active} className="row" onClick={onClick}>
      <span className="folder-radio" aria-hidden />
      <span className="row-main">
        <span className="row-title" style={{ display: 'block' }}>{title}</span>
        <span className="row-sub" style={{ display: 'block' }}>{desc}</span>
      </span>
    </button>
  )
}
