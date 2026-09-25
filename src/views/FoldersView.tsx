import { MOBILE_UI } from '../lib/platform'
import { folderName as baseFolderName } from '../lib/syncedFolders'
import { useEffect, useState, type ReactNode } from 'react'
import { AnimatePresence } from 'framer-motion'
import { ShareCode } from '../components/CodeQr'
import {
  AlertCircle,
  CheckCircle2,
  Clock,
  Folder,
  FolderCheck,
  FolderOpen,
  FolderSync,
  History,
  QrCode,
  Settings2,
  Unlink,
  UserMinus,
  UserPlus,
  WifiOff,
} from 'lucide-react'
import {
  api,
  type FolderStatus,
  type Pair,
  type PairUpdate,
  type VerifyResult,
} from '../lib/api'
import { useStore } from '../store'
import { formatBytes, formatEta, formatRelativeTime, formatSpeed as formatSpeedValue } from '../lib/format'
import { baseName } from '../lib/humanize'
import { PairingModal } from '../components/PairingModal'
import { Dialog } from '../components/Dialog'
import { avatarColor, initials } from '../lib/avatar'
import { FriendAvatar } from '../components/FriendAvatar'
import { friendOnlineState } from '../lib/presence'
import { ConnInfo } from '../components/ConnInspector'
import {
  Dot,
  EmptyState,
  IconButton,
  MenuButton,
  ProgressBar,
  Segmented,
  SectionHeader,
  Spinner,
  Toggle,
  type MenuItem,
} from '../components/ui'

// ── Glyphs ────────────────────────────────────────────────────────────────────
// Filled pause/play (SF "pause.fill"/"play.fill" style): lucide's outlined pause
// reads as a "columns" icon at 16px.
function PauseGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden fill="currentColor">
      <rect x="4" y="3" width="2.75" height="10" rx="0.9" />
      <rect x="9.25" y="3" width="2.75" height="10" rx="0.9" />
    </svg>
  )
}
function PlayGlyph() {
  return (
    <svg viewBox="0 0 16 16" width="16" height="16" aria-hidden fill="currentColor">
      <path d="M5 3.4v9.2c0 .6.66.97 1.17.65l7.2-4.6a.77.77 0 0 0 0-1.3l-7.2-4.6A.77.77 0 0 0 5 3.4Z" />
    </svg>
  )
}

export function FoldersView() {
  const pairs = useStore((s) => s.pairs)
  const statuses = useStore((s) => s.folderStatuses)
  const [modal, setModal] = useState<'create' | 'accept' | null>(null)
  // A folder invite scanned/pasted somewhere else (Receive box, Add friend, the
  // menu-bar popover) lands here: Accept invite opens with it prefilled.
  const pendingInvite = useStore((s) => s.pendingFolderInvite)
  const setPendingInvite = useStore((s) => s.setPendingFolderInvite)
  const [invite, setInvite] = useState<{ code: string; name: string } | null>(null)

  return (
    <div className="page">
      <div className="page-header titlebar-drag">
        <h1 className="page-title">Shared Folders</h1>
        <div className="page-actions">
          <button className="btn btn-secondary" onClick={() => setModal('accept')}>
            Accept invite…
          </button>
          <button className="btn btn-primary" onClick={() => setModal('create')}>
            New folder…
          </button>
        </div>
      </div>

      {pairs.length === 0 ? (
        <EmptyState
          icon={<FolderSync />}
          title="No shared folders"
          hint="Keep a folder in sync with friends."
        />
      ) : (
        <div className="group folder-list">
          {groupFolders(pairs).map((g) => (
            <FolderRow
              key={g.key}
              pair={g.rep}
              members={g.members}
              statuses={statuses}
              onShowInvite={(code) => setInvite({ code, name: baseFolderName(g.rep.folder) })}
            />
          ))}
        </div>
      )}

      <AnimatePresence>
        {(modal || pendingInvite) && (
          <PairingModal
            key={pendingInvite ?? modal ?? ''}
            mode={pendingInvite ? 'accept' : modal!}
            initialInvite={pendingInvite ?? ''}
            onClose={() => { setModal(null); setPendingInvite(null) }}
          />
        )}
      </AnimatePresence>
      <AnimatePresence>
        {invite && <InviteModal code={invite.code} folderName={invite.name} onClose={() => setInvite(null)} />}
      </AnimatePresence>
    </div>
  )
}

/** Collapse the pairwise links into one entry per shared folder: a 1:1 folder is
 *  its own group; a multi-person folder gathers all its links under group_id. */
function groupFolders(pairs: Pair[]): { key: string; rep: Pair; members: Pair[] }[] {
  const byKey = new Map<string, Pair[]>()
  for (const p of pairs) {
    const key = p.groupId ?? p.id
    const arr = byKey.get(key) ?? []
    arr.push(p)
    byKey.set(key, arr)
  }
  return [...byKey.entries()].map(([key, members]) => ({ key, rep: members[0], members }))
}

/** "Alex" · "Alex, Sam" · "Alex, Sam and 2 others". */
function joinNames(names: string[]): string {
  if (names.length <= 2) return names.join(', ')
  if (names.length === 3) return `${names[0]}, ${names[1]}, ${names[2]}`
  return `${names[0]}, ${names[1]} and ${names.length - 2} others`
}

type Tone = 'ok' | 'busy' | 'warn' | 'error' | 'off'

/** One status, in words: "Syncing 8 of 12 · 41 MB/s", "Up to date", "Paused"… */
function statusInfo(
  pair: Pair,
  status: FolderStatus | undefined,
  formatSpeed: (bps: number) => string,
): { tone: Tone; label: string } {
  const peer = pair.peerName || 'your friend'
  if (status?.peerUnshared) {
    return { tone: 'error', label: `${pair.peerName || 'They'} stopped sharing this folder` }
  }
  if (status?.paused) return { tone: 'off', label: 'Paused' }
  // Only truly "waiting" if the creator has never been reached by anyone yet.
  if (pair.role === 'a' && !pair.peerName && !status?.peerOnline) {
    return { tone: 'warn', label: 'Waiting for someone to join' }
  }
  const st = status?.state ?? 'idle'
  const speed = status && status.speedBps > 0 ? ` · ${formatSpeed(status.speedBps)}` : ''
  switch (st) {
    case 'sending': {
      const total = status?.sessionTotalFiles ?? 0
      const done = status?.sessionDoneFiles ?? 0
      return {
        tone: 'busy',
        label: total > 1 ? `Syncing ${Math.min(done + 1, total)} of ${total}${speed}` : `Sending${speed}`,
      }
    }
    case 'receiving':
      return { tone: 'busy', label: `Receiving${speed}` }
    case 'waiting':
      // The Rust sender fills `detail` with the honest, specific reason: either
      // "Waiting for <peer> to come online" (peer offline) or "Couldn't reach <peer>
      // just now — retrying" (reached but the transfer failed/stalled).
      return {
        tone: 'warn',
        label:
          (status?.detail ?? `Waiting for ${peer}`) +
          (status && status.queued > 0 ? ` · ${status.queued} queued` : ''),
      }
    case 'error':
      return { tone: 'error', label: status?.detail ?? 'Something went wrong' }
    default:
      if (status && !status.peerOnline && pair.peerName) {
        return { tone: 'off', label: `${peer} is offline` }
      }
      return { tone: 'ok', label: 'Up to date' }
  }
}

/** "Just now" / "Today 3:42 PM" read as "synced just now" mid-sentence (month names keep their case). */
const midSentence = (rel: string) => (/^(Just|Today|Yesterday)\b/.test(rel) ? rel[0].toLowerCase() + rel.slice(1) : rel)

function useFolderSound(pairId: string) {
  const [soundOn, setSoundOn] = useState(() => {
    try {
      return localStorage.getItem(`folder-sound-${pairId}`) === 'on'
    } catch {
      return false
    }
  })
  const toggle = () => {
    const next = !soundOn
    setSoundOn(next)
    try {
      localStorage.setItem(`folder-sound-${pairId}`, next ? 'on' : 'off')
    } catch {
      /* localStorage unavailable — the toggle just won't persist */
    }
  }
  return [soundOn, toggle] as const
}

function FolderRow({
  pair,
  members,
  statuses,
  onShowInvite,
}: {
  pair: Pair
  members: Pair[]
  statuses: Record<string, FolderStatus>
  onShowInvite: (code: string) => void
}) {
  const updatePair = useStore((s) => s.updatePair)
  const showMegabits = useStore((s) => s.settings?.showMegabits ?? false)
  const formatSpeed = (bps: number) => formatSpeedValue(bps, showMegabits)
  const removePair = useStore((s) => s.removePair)
  const reloadPairs = useStore((s) => s.reloadPairs)
  const toast = useStore((s) => s.toast)
  const focusFolderHistory = useStore((s) => s.focusFolderHistory)
  const status = statuses[pair.id]
  const lastSynced = useStore((s) => s.folderLastSynced[pair.id])
  const summary = useStore((s) => s.folderSummaries[pair.id])
  const myEid = useStore((s) => s.myEid)
  // We own this folder iff its recorded owner endpoint id is OURS.
  const iAmOwner = !!myEid && !!pair.ownerEid && pair.ownerEid === myEid
  const isGroup = members.length > 1 || !!pair.groupId
  // Settings + unpair apply to the WHOLE folder (every member link).
  const updateGroup = (patch: Partial<PairUpdate>) =>
    members.forEach((m) => updatePair({ ...patch, id: m.id }))
  const removeGroup = () => members.forEach((m) => removePair(m.id))

  const [dialog, setDialog] = useState<'settings' | 'add' | 'unpair' | null>(null)
  const [addingPerson, setAddingPerson] = useState(false)
  // "Share an invite code" path: mint a fresh group invite anyone can use.
  const addPerson = async () => {
    setAddingPerson(true)
    try {
      onShowInvite(await api.folderAddPerson(pair.id))
    } catch (e) {
      toast('error', String(e))
    } finally {
      setAddingPerson(false)
    }
  }
  const [loadingInvite, setLoadingInvite] = useState(false)
  const showInvite = async () => {
    setLoadingInvite(true)
    try {
      onShowInvite(await api.pairInvite(pair.id))
    } catch (e) {
      toast('error', String(e))
    } finally {
      setLoadingInvite(false)
    }
  }

  // Owner only: set a member to editor (full) or viewer (read-only). No-op if
  // they're already that role.
  const setRole = async (m: Pair, viewer: boolean) => {
    if (!!m.peerIsViewer === viewer) return
    try {
      await api.setMemberRole(m.id, viewer)
      await reloadPairs()
    } catch (e) {
      toast('error', String(e))
    }
  }

  const [verifying, setVerifying] = useState(false)
  // The last Verify outcome, shown in the row as a clear confirmation. null = not run.
  const [verifyResult, setVerifyResult] = useState<VerifyResult | null>(null)
  const runVerify = async () => {
    setVerifying(true)
    setVerifyResult(null)
    try {
      setVerifyResult(await api.verifyFolder(pair.id))
    } catch (e) {
      toast('error', String(e))
    } finally {
      setVerifying(false)
    }
  }
  // The confirmation fades after a while; the folder's status line takes over.
  useEffect(() => {
    if (!verifyResult) return
    const t = window.setTimeout(() => setVerifyResult(null), 15_000)
    return () => window.clearTimeout(t)
  }, [verifyResult])

  const folderName = baseFolderName(pair.folder) // Windows paths use backslashes
  const names = members.map((m) => m.peerName).filter(Boolean)
  const pendingCount = members.length - names.length
  const withLine = names.length
    ? `with ${joinNames(names)}${pendingCount ? ` · ${pendingCount} invite${pendingCount === 1 ? '' : 's'} pending` : ''}`
    : 'Just you so far'
  // This side only receives: a viewer, or the receiving end of a one-way folder.
  const viewOnly = !!pair.iAmViewer || (!pair.mirror && !pair.twoWay && pair.role === 'b')

  const info = statusInfo(pair, status, formatSpeed)
  const active = status?.state === 'sending' || status?.state === 'receiving'
  const pendingInvite = pair.role === 'a' && !pair.peerName
  let statusExtra = ''
  if (info.tone === 'ok' && summary && summary.files > 0) {
    statusExtra = `${summary.direction === 'send' ? 'Sent' : 'Received'} ${summary.files} file${summary.files === 1 ? '' : 's'} · ${formatBytes(summary.bytes)} in ${formatEta(summary.durationMs / 1000)} · ${formatSpeed(summary.avgBps)} avg`
  }
  const statusTitle = lastSynced ? `Last synced ${midSentence(formatRelativeTime(lastSynced))}` : undefined

  const menu: MenuItem[] = [
    { label: 'Add person…', icon: <UserPlus />, onSelect: () => setDialog('add'), disabled: addingPerson },
    { label: 'Show invite…', icon: <QrCode />, onSelect: () => void showInvite(), hidden: pair.role !== 'a', disabled: loadingInvite },
    { separator: true },
    { label: 'Folder history', icon: <History />, onSelect: () => focusFolderHistory(pair.id), hidden: !pair.mirror },
    { label: 'Verify', icon: <FolderCheck />, onSelect: () => void runVerify(), hidden: !pair.mirror, disabled: verifying },
    { label: 'Settings…', icon: <Settings2 />, onSelect: () => setDialog('settings') },
    { separator: true },
    { label: isGroup ? 'Leave folder…' : 'Unpair…', icon: <Unlink />, danger: true, onSelect: () => setDialog('unpair') },
  ]

  return (
    <div className="row folder-row">
      <span className="folder-glyph" aria-hidden><Folder /></span>
      <div className="row-main">
        <div className="row-title truncate-1" title={pair.folder}>{folderName}</div>
        <div className="row-sub truncate-1" title={withLine}>{withLine}</div>
        <div className={`folder-status tone-${info.tone}`} title={statusTitle}>
          <Dot tone={info.tone} />
          <span className="truncate-1 tnum">
            {info.label}
            {viewOnly && <span className="folder-status-quiet"> · View only</span>}
            {statusExtra && <span className="folder-status-quiet"> · {statusExtra}</span>}
          </span>
          {pendingInvite && (
            <button className="btn btn-plain btn-sm folder-status-action" onClick={showInvite} disabled={loadingInvite}>
              Show invite…
            </button>
          )}
        </div>

        {(verifying || verifyResult) && <VerifyLine verifying={verifying} result={verifyResult} />}

        {/* Live transfer: the file, a thin bar, bytes + time left. */}
        {active && status && (
          <div className="folder-xfer">
            <div className="folder-xfer-head">
              <span className="folder-xfer-name truncate-1" title={status.sendingFile ?? undefined}>
                {status.sendingFile ? baseName(status.sendingFile) : status.state === 'sending' ? 'Sending…' : 'Receiving…'}
              </span>
              <span className="folder-xfer-meta tnum">
                {status.bytesTotal > 0
                  ? `${formatBytes(status.bytesDone)} of ${formatBytes(status.bytesTotal)}`
                  : formatBytes(status.bytesDone)}
                {status.etaSeconds != null ? ` · ${formatEta(status.etaSeconds)} left` : ''}
              </span>
              <ConnInfo detail={status.connDetail} locality={status.locality} />
              {status.state === 'sending' && (
                <button
                  className="btn btn-secondary btn-sm"
                  title="Stop this transfer — it isn’t lost, it tries again later"
                  onClick={() => api.stopFolderTransfer(pair.id)}
                >
                  Stop
                </button>
              )}
            </div>
            <ProgressBar percent={status.percent} label={`${folderName} progress`} />
            {status.queuedFiles && status.queuedFiles.length > 0 && (
              <div className="folder-queue truncate-1" title={status.queuedFiles.join('\n')}>
                {status.queuedFiles.length} more queued: {status.queuedFiles.map((n) => baseName(n)).join(', ')}
              </div>
            )}
          </div>
        )}
      </div>

      <div className="row-trailing folder-actions">
        {!MOBILE_UI && (
          <IconButton label="Open folder" tooltip="Show in Finder" onClick={() => api.openPath(pair.folder)}>
            <FolderOpen />
          </IconButton>
        )}
        {pair.mirror && (
          <IconButton
            label={status?.paused ? 'Resume syncing' : 'Pause syncing'}
            onClick={() => void api.setFolderPaused(pair.id, !status?.paused)}
          >
            {status?.paused ? <PlayGlyph /> : <PauseGlyph />}
          </IconButton>
        )}
        <MenuButton label="More" items={menu} />
      </div>

      <AnimatePresence>
        {dialog === 'settings' && (
          <FolderSettingsDialog
            key="settings"
            pair={pair}
            members={members}
            statuses={statuses}
            folderName={folderName}
            iAmOwner={iAmOwner}
            isGroup={isGroup}
            viewOnly={viewOnly}
            onUpdate={updateGroup}
            onSetRole={setRole}
            onRemoveMember={(id) => removePair(id)}
            onAddPerson={() => setDialog('add')}
            onUnpair={() => setDialog('unpair')}
            onClose={() => setDialog(null)}
          />
        )}
        {dialog === 'add' && (
          <AddPersonDialog
            key="add"
            pair={pair}
            members={members}
            folderName={folderName}
            onClose={() => setDialog(null)}
            onShareCode={() => { setDialog(null); void addPerson() }}
          />
        )}
        {dialog === 'unpair' && (
          <Dialog
            key="unpair"
            title={isGroup ? `Leave “${folderName}”?` : `Unpair “${folderName}”?`}
            width={380}
            onClose={() => setDialog(null)}
            footer={
              <>
                <button className="btn btn-secondary" onClick={() => setDialog(null)}>Cancel</button>
                <button className="btn btn-destructive" onClick={() => { setDialog(null); removeGroup() }}>
                  {isGroup ? 'Leave' : 'Unpair'}
                </button>
              </>
            }
          >
            <p className="dialog-text" style={{ margin: 0 }}>
              {isGroup
                ? 'You’ll stop syncing this folder with everyone.'
                : `Syncing with ${pair.peerName || 'the other person'} stops.`}{' '}
              Files already here stay on this computer.
            </p>
          </Dialog>
        )}
      </AnimatePresence>
    </div>
  )
}

/** The Verify outcome: a spinner while the manifest round-trip runs, then a clear
 *  confirmation of whether the two folders are identical. */
function VerifyLine({ verifying, result }: { verifying: boolean; result: VerifyResult | null }) {
  let icon: ReactNode
  let text: string
  let tone = ''
  if (verifying || !result) {
    icon = <Spinner size={12} />
    text = 'Checking both folders match…'
  } else if (!result.compared) {
    icon = <WifiOff />
    tone = 'warn'
    text = result.peerOnline
      ? 'Couldn’t check yet — try Verify again in a moment'
      : 'Couldn’t check — the other device is offline'
  } else if (result.identical) {
    icon = <CheckCircle2 />
    tone = 'ok'
    text = `Both folders match · ${result.matched.toLocaleString()} ${result.matched === 1 ? 'file' : 'files'}`
  } else {
    const d = result.differences
    const parts: string[] = []
    if (result.missingOnPeer) parts.push(`${result.missingOnPeer} to send`)
    if (result.missingLocally) parts.push(`${result.missingLocally} to receive`)
    if (result.pendingDeletes) parts.push(`${result.pendingDeletes} to remove`)
    icon = <AlertCircle />
    tone = 'warn'
    text = `Found ${d} ${d === 1 ? 'difference' : 'differences'}${parts.length ? ` (${parts.join(', ')})` : ''} · fixing now`
  }
  return (
    <div className={`folder-verify ${tone}`} role="status">
      {icon}
      <span className="truncate-1">{text}</span>
    </div>
  )
}

function FolderSettingsDialog({
  pair,
  members,
  statuses,
  folderName,
  iAmOwner,
  isGroup,
  viewOnly,
  onUpdate,
  onSetRole,
  onRemoveMember,
  onAddPerson,
  onUnpair,
  onClose,
}: {
  pair: Pair
  members: Pair[]
  statuses: Record<string, FolderStatus>
  folderName: string
  iAmOwner: boolean
  isGroup: boolean
  viewOnly: boolean
  onUpdate: (patch: Partial<PairUpdate>) => void
  onSetRole: (m: Pair, viewer: boolean) => void
  onRemoveMember: (id: string) => Promise<void> | void
  onAddPerson: () => void
  onUnpair: () => void
  onClose: () => void
}) {
  const myName = useStore((s) => s.settings?.displayName || 'You')
  const [soundOn, toggleSound] = useFolderSound(pair.id)
  // Per-member removal (incl. clearing a stuck "waiting to join" invite).
  const [confirmMember, setConfirmMember] = useState<string | null>(null)
  const memberToRemove = members.find((m) => m.id === confirmMember)
  const doRemoveMember = async () => {
    if (!confirmMember) return
    try {
      await onRemoveMember(confirmMember)
    } finally {
      setConfirmMember(null)
    }
  }
  const myRole = iAmOwner ? 'Owner' : viewOnly ? 'Viewer' : 'Editor'

  return (
    <Dialog
      title={folderName}
      width={460}
      className="folder-dialog"
      onClose={onClose}
      footer={
        <>
          <button className="btn btn-danger" onClick={onUnpair}>
            {isGroup ? 'Leave folder…' : 'Unpair…'}
          </button>
          <span className="spacer" />
          <button className="btn btn-primary" onClick={onClose}>Done</button>
        </>
      }
    >
      <SectionHeader style={{ marginTop: 0 }}>People</SectionHeader>
      <div className="group folder-people">
        <div className="row">
          <span className="folder-avatar" style={{ background: avatarColor(myName) }}>{initials(myName)}</span>
          <div className="row-main">
            <div className="row-title truncate-1">{myName} <span className="faint">(you)</span></div>
            {viewOnly && <div className="row-sub">Changes you make here aren’t sent</div>}
          </div>
          <span className="folder-role">{myRole}</span>
        </div>
        {members.map((m) => {
          const pending = !m.peerName
          const online = statuses[m.id]?.peerOnline ?? false
          const canSetRole = iAmOwner && !pending
          return (
            <div className="row" key={m.id}>
              <span
                className={`folder-avatar${pending ? ' pending' : ''}`}
                style={pending ? undefined : { background: avatarColor(m.peerName) }}
              >
                {pending ? <Clock /> : initials(m.peerName)}
                {!pending && <span className={`folder-avatar-dot${online ? ' online' : ''}`} />}
              </span>
              <div className="row-main">
                <div className="row-title truncate-1" title={m.peerName || undefined}>
                  {pending ? 'Invite pending' : m.peerName}
                </div>
                <div className="row-sub">{pending ? 'Waiting for them to join' : online ? 'Online' : 'Offline'}</div>
              </div>
              {/* Only the folder OWNER may assign roles (enforced in the engine too). */}
              {canSetRole ? (
                <Segmented
                  label={`${m.peerName}’s access`}
                  value={m.peerIsViewer ? 'viewer' : 'editor'}
                  options={[
                    { value: 'editor', label: 'Editor', title: 'Can add, change and delete files' },
                    { value: 'viewer', label: 'Viewer', title: 'Can view and download, but not change the folder' },
                  ]}
                  onChange={(v) => onSetRole(m, v === 'viewer')}
                />
              ) : (
                !pending && <span className="folder-role">{m.peerIsViewer ? 'Viewer' : 'Editor'}</span>
              )}
              <IconButton
                size="sm"
                danger
                label={pending ? 'Cancel invite' : `Remove ${m.peerName}`}
                onClick={() => setConfirmMember(m.id)}
              >
                <UserMinus />
              </IconButton>
            </div>
          )
        })}
        <button type="button" className="row folder-add-row" onClick={onAddPerson}>
          <span className="folder-avatar add"><UserPlus /></span>
          <span className="row-main row-title">Add person…</span>
        </button>
      </div>

      <SectionHeader>Sync</SectionHeader>
      <div className="group folder-settings">
        <SettingRow title="Total sync" desc="Adds, edits and deletes sync both ways. Removed files are kept in History.">
          <Toggle label="Total sync" on={pair.mirror} onChange={() => onUpdate({ mirror: !pair.mirror })} />
        </SettingRow>
        {!pair.mirror && (
          <SettingRow title="Two-way" desc="Receive their files too, not just send.">
            <Toggle label="Two-way sync" on={pair.twoWay} onChange={() => onUpdate({ twoWay: !pair.twoWay })} />
          </SettingRow>
        )}
        {!pair.mirror && (
          <SettingRow title="Delete after delivery" desc="Remove your copy once they have it.">
            <Toggle label="Delete after delivery" on={pair.autoDelete} onChange={() => onUpdate({ autoDelete: !pair.autoDelete })} />
          </SettingRow>
        )}
        {!pair.mirror && pair.autoDelete && (
          <SettingRow title="When deleting" desc="Trash can be recovered; permanent can’t.">
            <Segmented
              label="When deleting"
              value={pair.deleteMode}
              options={[
                { value: 'trash', label: 'Trash' },
                { value: 'permanent', label: 'Permanent' },
              ]}
              onChange={(v) => onUpdate({ deleteMode: v })}
            />
          </SettingRow>
        )}
        <SettingRow title="Play sound on sync" desc="A soft sound when files send or arrive.">
          <Toggle label="Play sound on sync" on={soundOn} onChange={toggleSound} />
        </SettingRow>
      </div>

      <AnimatePresence>
        {memberToRemove && (
          <Dialog
            key="remove-member"
            title={memberToRemove.peerName ? `Remove ${memberToRemove.peerName}?` : 'Cancel this invite?'}
            width={380}
            onClose={() => setConfirmMember(null)}
            footer={
              <>
                <button className="btn btn-secondary" onClick={() => setConfirmMember(null)}>
                  {memberToRemove.peerName ? 'Cancel' : 'Keep invite'}
                </button>
                <button className="btn btn-destructive" onClick={doRemoveMember}>
                  {memberToRemove.peerName ? 'Remove' : 'Cancel invite'}
                </button>
              </>
            }
          >
            <p className="dialog-text" style={{ margin: 0 }}>
              {memberToRemove.peerName
                ? `They’ll stop syncing “${folderName}” with you.`
                : 'Anyone you sent the invite to won’t be able to join with it.'}
            </p>
          </Dialog>
        )}
      </AnimatePresence>
    </Dialog>
  )
}

function SettingRow({ title, desc, children }: { title: string; desc: string; children: ReactNode }) {
  return (
    <div className="row">
      <div className="row-main">
        <div className="row-title">{title}</div>
        <div className="row-sub">{desc}</div>
      </div>
      <div className="row-trailing">{children}</div>
    </div>
  )
}

/** Add someone to an existing folder: pick a friend (they get an in-app prompt,
 *  no code needed) or fall back to sharing an invite code with anyone. */
function AddPersonDialog({
  pair,
  members,
  folderName,
  onClose,
  onShareCode,
}: {
  pair: Pair
  members: Pair[]
  folderName: string
  onClose: () => void
  onShareCode: () => void
}) {
  const friends = useStore((s) => s.friends)
  const friendSeen = useStore((s) => s.friendSeen)
  const folderStatuses = useStore((s) => s.folderStatuses)
  const reloadPairs = useStore((s) => s.reloadPairs)
  const toast = useStore((s) => s.toast)
  const [busy, setBusy] = useState<string | null>(null)
  // Already in the folder = same device (endpoint id) or, for older links, same name.
  const memberEids = new Set(members.map((m) => m.endpointId).filter(Boolean))
  const memberNames = new Set(members.map((m) => (m.peerName ?? '').trim().toLowerCase()).filter(Boolean))
  const candidates = friends.filter(
    (f) => !(f.endpointId && memberEids.has(f.endpointId)) && !memberNames.has(f.name.trim().toLowerCase()),
  )
  const invite = async (friendId: string, name: string) => {
    setBusy(friendId)
    try {
      await api.inviteFriendToFolder(pair.id, friendId, null)
      await reloadPairs()
      const online = friendOnlineState(name, friendSeen, folderStatuses) === true
      toast(online ? 'success' : 'info', online
        ? `Invited ${name} to “${folderName}”`
        : `${name} is offline. If the invite doesn’t arrive, share a code instead.`)
      onClose()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(null)
    }
  }
  return (
    <Dialog
      title={`Add someone to “${folderName}”`}
      width={420}
      className="folder-dialog"
      onClose={onClose}
      busy={!!busy}
      footer={
        <>
          <button className="btn btn-secondary" disabled={!!busy} onClick={onShareCode}>
            <QrCode /> Share an invite code…
          </button>
          <span className="spacer" />
          <button className="btn btn-secondary" disabled={!!busy} onClick={onClose}>Cancel</button>
        </>
      }
    >
      {candidates.length === 0 ? (
        <p className="dialog-text" style={{ margin: 0 }}>
          {friends.length ? 'All your friends are already in this folder.' : 'You haven’t added any friends yet.'}
        </p>
      ) : (
        <div className="group folder-pick">
          {candidates.map((f) => {
            const online = friendOnlineState(f.name, friendSeen, folderStatuses) === true
            return (
              <button key={f.id} type="button" className="row" disabled={!!busy} onClick={() => void invite(f.id, f.name)}>
                <span className="folder-avatar" style={{ background: avatarColor(f.id) }}>
                  <FriendAvatar friend={f} />
                  <span className={`folder-avatar-dot${online ? ' online' : ''}`} />
                </span>
                <span className="row-main">
                  <span className="row-title truncate-1" style={{ display: 'block' }}>{f.name}</span>
                  <span className="row-sub" style={{ display: 'block' }}>{online ? 'Online' : 'Offline'}</span>
                </span>
                {busy === f.id ? <Spinner size={14} /> : <span className="folder-pick-cta">Invite</span>}
              </button>
            )
          })}
        </div>
      )}
    </Dialog>
  )
}

function InviteModal({
  code,
  folderName,
  onClose,
}: {
  code: string
  folderName: string
  onClose: () => void
}) {
  return (
    <Dialog
      title={`Invite to “${folderName}”`}
      width={420}
      onClose={onClose}
      footer={<button className="btn btn-secondary" onClick={onClose}>Done</button>}
    >
      <p className="dialog-text">In DropBeam, they choose Accept invite and scan or paste this.</p>
      <ShareCode code={code} layout="stack" copyLabel="Copy invite" />
    </Dialog>
  )
}
