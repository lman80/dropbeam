import { useCallback, useEffect, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { QRCodeSVG } from 'qrcode.react'
import {
  ArrowLeftRight,
  ArrowRight,
  Check,
  Clock,
  Copy,
  FolderOpen,
  FolderSync,
  History,
  Plus,
  QrCode,
  RotateCcw,
  Settings2,
  Trash2,
  Unlink,
  UserPlus,
  X,
} from 'lucide-react'
import {
  api,
  onFolderHistoryChanged,
  type FolderStatus,
  type HistoryItem,
  type Pair,
  type PairUpdate,
} from '../lib/api'
import { useStore } from '../store'
import { EmptyState, LocalityBadge, ProgressBar, Spinner } from '../components/bits'
import { formatBytes, formatEta, formatRelativeTime, formatSpeed } from '../lib/format'
import { PairingModal } from '../components/PairingModal'
import { avatarGradient, initials } from '../lib/avatar'

export function FoldersView() {
  const pairs = useStore((s) => s.pairs)
  const statuses = useStore((s) => s.folderStatuses)
  const [modal, setModal] = useState<'create' | 'accept' | null>(null)
  const [invite, setInvite] = useState<{ code: string; name: string } | null>(null)

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
        <h1 style={{ fontSize: 20, fontWeight: 750, margin: 0 }}>Shared Folders</h1>
        <div style={{ display: 'flex', gap: 8 }}>
          <button className="btn btn-ghost" onClick={() => setModal('accept')}>
            <Plus size={15} /> Accept invite
          </button>
          <button className="btn btn-primary" onClick={() => setModal('create')}>
            <FolderSync size={15} /> New folder
          </button>
        </div>
      </div>

      {pairs.length === 0 ? (
        <div className="card">
          <EmptyState
            icon={<FolderSync size={24} />}
            title="No shared folders yet"
            hint="Pair a folder with a friend so anything you drop in is automatically beamed to them — even across the internet. Optionally, files vanish from your side once delivered."
          />
          <div style={{ display: 'flex', gap: 10, justifyContent: 'center', paddingBottom: 24 }}>
            <button className="btn btn-ghost" onClick={() => setModal('accept')}>
              Accept an invite
            </button>
            <button className="btn btn-primary" onClick={() => setModal('create')}>
              <FolderSync size={15} /> Create one
            </button>
          </div>
        </div>
      ) : (
        <div style={{ display: 'flex', flexDirection: 'column', gap: 12 }}>
          <AnimatePresence initial={false}>
            {groupFolders(pairs).map((g) => (
              <FolderCard
                key={g.key}
                pair={g.rep}
                members={g.members}
                statuses={statuses}
                onShowInvite={(code) =>
                  setInvite({ code, name: g.rep.folder.split('/').pop() || '' })
                }
              />
            ))}
          </AnimatePresence>
        </div>
      )}

      {modal && <PairingModal mode={modal} onClose={() => setModal(null)} />}
      {invite && <InviteModal code={invite.code} folderName={invite.name} onClose={() => setInvite(null)} />}
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

function statusInfo(
  pair: Pair,
  status?: FolderStatus,
  lastSynced?: number,
): { color: string; label: string } {
  const peer = pair.peerName || 'your friend'
  // Only truly "waiting" if the creator has never been reached by anyone yet.
  if (pair.role === 'a' && !pair.peerName && !status?.peerOnline) {
    return { color: 'var(--amber)', label: 'Waiting for someone to accept the invite' }
  }
  const st = status?.state ?? 'idle'
  switch (st) {
    case 'sending':
      return {
        color: 'var(--accent)',
        label: status?.sendingFile ? `Sending ${status.sendingFile}` : 'Sending…',
      }
    case 'receiving':
      return { color: 'var(--accent)', label: 'Receiving…' }
    case 'waiting':
      return {
        color: 'var(--amber)',
        label:
          (status?.detail ?? 'Waiting for the other device') +
          (status && status.queued > 0 ? ` · ${status.queued} queued` : ''),
      }
    case 'error':
      return { color: 'var(--red)', label: status?.detail ?? 'Something went wrong' }
    default:
      if (status && !status.peerOnline && pair.peerName) {
        return { color: 'var(--text-faint)', label: `${peer} is offline — will sync when they're back` }
      }
      return {
        color: 'var(--green)',
        label: lastSynced ? `Up to date · synced ${formatRelativeTime(lastSynced)}` : 'Up to date',
      }
  }
}

function FolderCard({
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
  const removePair = useStore((s) => s.removePair)
  const toast = useStore((s) => s.toast)
  const myName = useStore((s) => s.settings?.displayName || 'You')
  const status = statuses[pair.id]
  const lastSynced = useStore((s) => s.folderLastSynced[pair.id])
  const isGroup = members.length > 1 || !!pair.groupId
  // Settings + unpair apply to the WHOLE folder (every member link).
  const updateGroup = (patch: Partial<PairUpdate>) =>
    members.forEach((m) => updatePair({ ...patch, id: m.id }))
  const removeGroup = () => members.forEach((m) => removePair(m.id))
  const [addingPerson, setAddingPerson] = useState(false)
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
  const [open, setOpen] = useState(false)
  const [confirmUnpair, setConfirmUnpair] = useState(false)
  // Per-member removal (incl. clearing a stuck "waiting to join" invite).
  const [confirmMember, setConfirmMember] = useState<string | null>(null)
  const memberToRemove = members.find((m) => m.id === confirmMember)
  const doRemoveMember = async () => {
    if (!confirmMember) return
    try {
      await removePair(confirmMember)
    } finally {
      setConfirmMember(null)
    }
  }
  const [loadingInvite, setLoadingInvite] = useState(false)
  const [historyOpen, setHistoryOpen] = useState(false)
  const [verifying, setVerifying] = useState(false)
  const [soundOn, setSoundOn] = useState(() => {
    try {
      return localStorage.getItem(`folder-sound-${pair.id}`) === 'on'
    } catch {
      return false
    }
  })
  const toggleSound = () => {
    const next = !soundOn
    setSoundOn(next)
    try {
      localStorage.setItem(`folder-sound-${pair.id}`, next ? 'on' : 'off')
    } catch {
      /* localStorage unavailable — the toggle just won't persist */
    }
  }

  const info = statusInfo(pair, status, lastSynced)
  const folderName = pair.folder.split('/').pop() || pair.folder
  const peer = pair.peerName || 'Pending peer'

  const showInvite = async () => {
    setLoadingInvite(true)
    try {
      const code = await api.pairInvite(pair.id)
      onShowInvite(code)
    } catch (e) {
      toast('error', String(e))
    } finally {
      setLoadingInvite(false)
    }
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
        <div
          style={{
            width: 40,
            height: 40,
            borderRadius: 12,
            display: 'grid',
            placeItems: 'center',
            flexShrink: 0,
            color: 'white',
            background: 'linear-gradient(135deg, var(--accent), var(--accent-2))',
          }}
        >
          <FolderSync size={19} />
        </div>
        <div style={{ flex: 1, minWidth: 0 }}>
          <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
            <span style={{ fontWeight: 700, fontSize: 14.5 }}>{peer}</span>
            {pair.mirror ? (
              <span className="chip" style={{ background: 'var(--accent-soft)', color: 'var(--accent)' }}>
                <FolderSync size={11} /> Total sync
              </span>
            ) : (
              <span
                className="chip"
                style={{ background: 'var(--surface-2)', color: 'var(--text-muted)' }}
              >
                {pair.twoWay ? <ArrowLeftRight size={11} /> : <ArrowRight size={11} />}
                {pair.twoWay
                  ? 'Two-way'
                  : pair.role === 'a'
                    ? 'View only (they receive)'
                    : 'View only (you receive)'}
              </span>
            )}
            {pair.autoDelete && (
              <span className="chip" style={{ background: 'var(--amber-soft)', color: 'var(--amber)' }}>
                Auto-delete
              </span>
            )}
          </div>
          <div
            style={{
              fontSize: 12,
              color: 'var(--text-faint)',
              marginTop: 2,
              overflow: 'hidden',
              textOverflow: 'ellipsis',
              whiteSpace: 'nowrap',
            }}
          >
            {folderName}
          </div>
        </div>
        <button className="icon-btn" title="Open folder" onClick={() => api.openPath(pair.folder)}>
          <FolderOpen size={16} />
        </button>
        <button
          className="icon-btn"
          title="Folder settings"
          onClick={() => setOpen((o) => !o)}
          style={{ color: open ? 'var(--accent)' : undefined }}
        >
          <Settings2 size={16} />
        </button>
      </div>

      <div style={{ display: 'flex', alignItems: 'center', gap: 8, marginTop: 12 }}>
        <span
          style={{
            width: 8,
            height: 8,
            borderRadius: 999,
            background: info.color,
            flexShrink: 0,
            boxShadow: `0 0 0 3px color-mix(in srgb, ${info.color} 22%, transparent)`,
          }}
        />
        <span style={{ fontSize: 13, color: 'var(--text-muted)' }}>{info.label}</span>
        {pair.role === 'a' && !pair.peerName && (
          <button
            className="btn btn-ghost"
            style={{ marginLeft: 'auto', padding: '5px 10px', fontSize: 12.5 }}
            onClick={showInvite}
            disabled={loadingInvite}
          >
            {loadingInvite ? <Spinner size={13} /> : <QrCode size={13} />} Show invite
          </button>
        )}
      </div>

      {/* The peer stopped sharing this folder — the link is effectively dead. */}
      {status?.peerUnshared && (
        <div
          style={{
            marginTop: 11,
            padding: '10px 12px',
            borderRadius: 10,
            background: 'color-mix(in srgb, var(--red) 12%, transparent)',
            border: '1px solid color-mix(in srgb, var(--red) 35%, transparent)',
            display: 'flex',
            alignItems: 'center',
            gap: 9,
            fontSize: 12.5,
          }}
        >
          <Unlink size={15} color="var(--red)" style={{ flexShrink: 0 }} />
          <span style={{ color: 'var(--text-muted)' }}>
            {(pair.peerName || 'The other person')} no longer shares this folder. Your files are
            still here — you can remove this now-inactive folder.
          </span>
        </div>
      )}

      {/* Members — everyone in this folder (you + each person you're linked to) */}
      <div style={{ display: 'flex', gap: 8, marginTop: 11, flexWrap: 'wrap', alignItems: 'center' }}>
        <Member name={myName} you online />
        {members.map((m) => (
          <Member
            key={m.id}
            name={m.peerName || 'Waiting to join…'}
            online={statuses[m.id]?.peerOnline ?? false}
            pending={!m.peerName}
            onRemove={() => setConfirmMember(m.id)}
          />
        ))}
        <button
          className="btn btn-ghost"
          style={{ padding: '5px 11px', fontSize: 12.5 }}
          onClick={addPerson}
          disabled={addingPerson}
          title="Invite another person to this folder"
        >
          {addingPerson ? <Spinner size={13} /> : <UserPlus size={13} />} Add person
        </button>
      </div>

      {/* Confirm removing one member (or clearing a stuck pending invite). */}
      {memberToRemove && (
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 10,
            marginTop: 9,
            padding: '9px 12px',
            background: 'var(--amber-soft)',
            border: '1px solid var(--border)',
            borderRadius: 10,
            fontSize: 12.5,
          }}
        >
          <span style={{ flex: 1, lineHeight: 1.4 }}>
            {memberToRemove.peerName
              ? `Remove ${memberToRemove.peerName} from this folder? They'll stop syncing it with you.`
              : 'Cancel this pending invite? Anyone you already sent the link to won’t be able to join with it.'}
          </span>
          <button className="btn btn-ghost" style={{ padding: '5px 10px' }} onClick={() => setConfirmMember(null)}>
            Cancel
          </button>
          <button className="btn btn-danger" style={{ padding: '5px 10px' }} onClick={doRemoveMember}>
            {memberToRemove.peerName ? 'Remove' : 'Cancel invite'}
          </button>
        </div>
      )}

      {/* Sync state — lets you confirm both folders actually match. */}
      {pair.mirror && status?.peerOnline && status?.state === 'idle' && (status?.queued ?? 0) === 0 && (
        <div
          style={{
            display: 'flex',
            alignItems: 'center',
            gap: 7,
            marginTop: 10,
            fontSize: 12.5,
            color: 'var(--text-muted)',
          }}
        >
          <Check size={14} color="var(--green)" style={{ flexShrink: 0 }} />
          <span>
            In sync
            {typeof status.peerFiles === 'number' && status.peerFiles > 0
              ? ` — ${pair.peerName || 'they'} ${pair.peerName ? 'has' : 'have'} ${status.peerFiles} file${status.peerFiles === 1 ? '' : 's'}`
              : ''}
          </span>
        </div>
      )}

      {/* Live transfer progress while sending or receiving */}
      {(status?.state === 'sending' || status?.state === 'receiving') && (
        <motion.div
          initial={{ opacity: 0, height: 0 }}
          animate={{ opacity: 1, height: 'auto' }}
          style={{ marginTop: 12, overflow: 'hidden' }}
        >
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              alignItems: 'baseline',
              marginBottom: 6,
            }}
          >
            <div style={{ display: 'flex', alignItems: 'center', gap: 8 }}>
              <span style={{ fontSize: 15, fontWeight: 750 }} className="gradient-text">
                {Math.round(status.percent)}%
              </span>
              <LocalityBadge locality={status.locality} />
            </div>
            <div style={{ display: 'flex', alignItems: 'center', gap: 10 }}>
              <span style={{ fontSize: 12, color: 'var(--text-muted)' }}>
                {formatBytes(status.bytesDone)}
                {status.bytesTotal > 0 ? ` / ${formatBytes(status.bytesTotal)}` : ''}
              </span>
              {status.state === 'sending' && (
                <button
                  className="btn btn-ghost"
                  title="Stop this transfer (it won't be lost — it retries)"
                  style={{ padding: '3px 9px', fontSize: 12 }}
                  onClick={() => api.stopFolderTransfer(pair.id)}
                >
                  <X size={13} /> Stop
                </button>
              )}
            </div>
          </div>
          <ProgressBar percent={status.percent} />
          <div
            style={{
              display: 'flex',
              justifyContent: 'space-between',
              fontSize: 11.5,
              color: 'var(--text-faint)',
              marginTop: 6,
              gap: 10,
            }}
          >
            <span
              style={{ overflow: 'hidden', textOverflow: 'ellipsis', whiteSpace: 'nowrap' }}
            >
              {status.sendingFile || (status.state === 'sending' ? 'Sending…' : 'Receiving…')}
            </span>
            <span style={{ flexShrink: 0 }}>
              {formatSpeed(status.speedBps)}
              {status.etaSeconds != null ? ` · ${formatEta(status.etaSeconds)} left` : ''}
            </span>
          </div>

          {/* The rest of a dropped batch, listed up front with per-file rows so it
              reads as "all these files are going through" — not one popup at a time. */}
          {status.queuedFiles && status.queuedFiles.length > 0 && (
            <div
              style={{
                marginTop: 10,
                display: 'flex',
                flexDirection: 'column',
                gap: 6,
                maxHeight: 168,
                overflowY: 'auto',
              }}
            >
              {status.queuedFiles.map((name, i) => (
                <div key={`${name}-${i}`} style={{ display: 'flex', flexDirection: 'column', gap: 3 }}>
                  <div
                    style={{
                      display: 'flex',
                      alignItems: 'center',
                      gap: 7,
                      fontSize: 12,
                      color: 'var(--text-faint)',
                    }}
                  >
                    <Clock size={12} style={{ flexShrink: 0 }} />
                    <span
                      style={{
                        overflow: 'hidden',
                        textOverflow: 'ellipsis',
                        whiteSpace: 'nowrap',
                      }}
                    >
                      {name}
                    </span>
                    <span style={{ marginLeft: 'auto', flexShrink: 0 }}>Queued</span>
                  </div>
                  <ProgressBar percent={0} />
                </div>
              ))}
            </div>
          )}
        </motion.div>
      )}

      <AnimatePresence initial={false}>
        {open && (
          <motion.div
            initial={{ height: 0, opacity: 0 }}
            animate={{ height: 'auto', opacity: 1 }}
            exit={{ height: 0, opacity: 0 }}
            transition={{ duration: 0.2 }}
            style={{ overflow: 'hidden' }}
          >
            <div style={{ borderTop: '1px solid var(--border)', marginTop: 14, paddingTop: 6 }}>
              <SettingRow
                title="Play sound on sync"
                desc="Hear a soft cue whenever a file is sent to or received from this folder. Off by default."
              >
                <button className={`toggle${soundOn ? ' on' : ''}`} onClick={toggleSound} />
              </SettingRow>
              <SettingRow
                title="Total sync (source of truth)"
                desc="Adds, edits, and deletes all sync both ways — like a shared drive. Deleted and replaced files are kept in History so nothing is lost."
              >
                <button
                  className={`toggle${pair.mirror ? ' on' : ''}`}
                  onClick={() => updateGroup({ mirror: !pair.mirror })}
                />
              </SettingRow>
              {!pair.mirror && (
                <SettingRow
                  title="Two-way sync"
                  desc="Receive the peer's files too, not just send."
                >
                  <button
                    className={`toggle${pair.twoWay ? ' on' : ''}`}
                    onClick={() => updateGroup({ twoWay: !pair.twoWay })}
                  />
                </SettingRow>
              )}
              {!pair.mirror && (
                <SettingRow
                  title="Delete after delivery"
                  desc="Remove the local copy once the peer confirms receipt — a self-emptying outbox."
                >
                  <button
                    className={`toggle${pair.autoDelete ? ' on' : ''}`}
                    onClick={() => updateGroup({ autoDelete: !pair.autoDelete })}
                  />
                </SettingRow>
              )}
              {!pair.mirror && pair.autoDelete && (
                <SettingRow title="When deleting" desc="Trash is recoverable; permanent is not.">
                  <div className="seg">
                    {(['trash', 'permanent'] as const).map((m) => (
                      <button
                        key={m}
                        className={pair.deleteMode === m ? 'active' : ''}
                        style={{ textTransform: 'capitalize', padding: '5px 12px' }}
                        onClick={() => updateGroup({ deleteMode: m })}
                      >
                        {m === 'trash' ? 'Trash' : 'Permanent'}
                      </button>
                    ))}
                  </div>
                </SettingRow>
              )}
              <div
                style={{
                  display: 'flex',
                  gap: 8,
                  justifyContent: 'flex-end',
                  marginTop: 12,
                  paddingBottom: 2,
                }}
              >
                {pair.mirror && (
                  <button
                    className="btn btn-ghost"
                    style={{ marginRight: 'auto' }}
                    onClick={() => setHistoryOpen(true)}
                  >
                    <History size={14} /> History
                  </button>
                )}
                {pair.mirror && (
                  <button
                    className="btn btn-ghost"
                    title="Re-check that both folders are identical and fix any difference"
                    disabled={verifying}
                    onClick={async () => {
                      setVerifying(true)
                      try {
                        await api.verifyFolders()
                      } catch {
                        /* best-effort */
                      }
                      setTimeout(() => setVerifying(false), 2500)
                    }}
                  >
                    {verifying ? <Spinner size={13} /> : <FolderSync size={14} />}{' '}
                    {verifying ? 'Checking…' : 'Verify'}
                  </button>
                )}
                {pair.role === 'a' && (
                  <button className="btn btn-ghost" onClick={showInvite} disabled={loadingInvite}>
                    <QrCode size={14} /> Show invite
                  </button>
                )}
                {confirmUnpair ? (
                  <>
                    <button className="btn btn-ghost" onClick={() => setConfirmUnpair(false)}>
                      Cancel
                    </button>
                    <button className="btn btn-danger" onClick={removeGroup}>
                      <Unlink size={14} /> Confirm {isGroup ? 'leave' : 'unpair'}
                    </button>
                  </>
                ) : (
                  <button className="btn btn-danger" onClick={() => setConfirmUnpair(true)}>
                    <Unlink size={14} /> Unpair
                  </button>
                )}
              </div>
            </div>
          </motion.div>
        )}
      </AnimatePresence>

      {historyOpen && (
        <HistoryModal pair={pair} onClose={() => setHistoryOpen(false)} />
      )}
    </motion.div>
  )
}

function HistoryModal({ pair, onClose }: { pair: Pair; onClose: () => void }) {
  const toast = useStore((s) => s.toast)
  const [items, setItems] = useState<HistoryItem[] | null>(null)
  const [busy, setBusy] = useState<string | null>(null)

  const load = useCallback(async () => {
    try {
      setItems(await api.listFolderHistory(pair.id))
    } catch (e) {
      toast('error', String(e))
      setItems([])
    }
  }, [pair.id, toast])

  useEffect(() => {
    let alive = true
    // load() is async — setState runs after the await, not during the effect.
    // eslint-disable-next-line react-hooks/set-state-in-effect
    void load()
    const un = onFolderHistoryChanged((pid) => {
      if (alive && pid === pair.id) void load()
    })
    return () => {
      alive = false
      un.then((f) => f())
    }
  }, [pair.id, load])

  const restore = async (item: HistoryItem) => {
    setBusy(item.id)
    try {
      await api.restoreFolderItem(pair.id, item.id)
      toast('success', `Restored ${item.relPath.split('/').pop()}`)
      await load()
    } catch (e) {
      toast('error', String(e))
    } finally {
      setBusy(null)
    }
  }

  const forget = async (item: HistoryItem) => {
    setBusy(item.id)
    try {
      await api.forgetFolderItem(pair.id, item.id)
      await load()
    } finally {
      setBusy(null)
    }
  }

  return (
    <div
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
        initial={{ opacity: 0, scale: 0.96 }}
        animate={{ opacity: 1, scale: 1 }}
        onClick={(e) => e.stopPropagation()}
        className="card"
        style={{ width: 520, maxWidth: '100%', maxHeight: '80vh', padding: 22, borderRadius: 20, display: 'flex', flexDirection: 'column' }}
      >
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 6 }}>
          <div style={{ fontWeight: 750, fontSize: 16 }}>Folder history</div>
          <button className="icon-btn" onClick={onClose}>
            <X size={17} />
          </button>
        </div>
        <p style={{ fontSize: 12.5, color: 'var(--text-muted)', marginTop: 0, lineHeight: 1.5 }}>
          Deleted and replaced files are kept here. Restore one and it comes back in the folder —
          and re-syncs to {pair.peerName || 'everyone'}.
        </p>

        <div style={{ overflowY: 'auto', marginTop: 8, display: 'flex', flexDirection: 'column', gap: 8 }}>
          {items === null ? (
            <div style={{ display: 'grid', placeItems: 'center', padding: 30 }}>
              <Spinner size={20} />
            </div>
          ) : items.length === 0 ? (
            <EmptyState icon={<Clock size={22} />} title="Nothing in history yet" hint="When a file is deleted or replaced in this folder, the old copy shows up here so you can get it back." />
          ) : (
            items.map((item) => (
              <div
                key={item.id}
                style={{
                  display: 'flex',
                  alignItems: 'center',
                  gap: 12,
                  padding: '10px 12px',
                  background: 'var(--surface-2)',
                  border: '1px solid var(--border)',
                  borderRadius: 12,
                }}
              >
                <div style={{ flex: 1, minWidth: 0 }}>
                  <div style={{ fontSize: 13.5, fontWeight: 600, whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                    {item.relPath}
                  </div>
                  <div style={{ fontSize: 11.5, color: 'var(--text-faint)', marginTop: 2 }}>
                    {item.reason === 'replaced' ? 'Replaced' : 'Deleted'} ·{' '}
                    {formatRelativeTime(item.timestampMs)} · {formatBytes(item.size)}
                  </div>
                </div>
                <button
                  className="btn btn-primary"
                  style={{ padding: '6px 12px' }}
                  onClick={() => restore(item)}
                  disabled={busy === item.id}
                >
                  {busy === item.id ? <Spinner size={13} /> : <RotateCcw size={14} />} Restore
                </button>
                <button
                  className="icon-btn"
                  title="Forget permanently"
                  onClick={() => forget(item)}
                  disabled={busy === item.id}
                >
                  <Trash2 size={15} />
                </button>
              </div>
            ))
          )}
        </div>
      </motion.div>
    </div>
  )
}

function Member({
  name,
  online,
  you,
  pending,
  onRemove,
}: {
  name: string
  online: boolean
  you?: boolean
  pending?: boolean
  onRemove?: () => void
}) {
  return (
    <div
      style={{
        display: 'flex',
        alignItems: 'center',
        gap: 7,
        padding: onRemove && !you ? '4px 6px 4px 5px' : '4px 11px 4px 5px',
        background: 'var(--surface-2)',
        borderRadius: 999,
        border: '1px solid var(--border)',
      }}
      title={you ? 'You' : online ? `${name} · online` : `${name} · offline`}
    >
      <span
        style={{
          position: 'relative',
          width: 24,
          height: 24,
          borderRadius: 999,
          display: 'grid',
          placeItems: 'center',
          color: 'white',
          fontWeight: 700,
          fontSize: 10,
          flexShrink: 0,
          background: you
            ? 'linear-gradient(135deg, var(--accent), var(--accent-2))'
            : pending
              ? 'var(--text-faint)'
              : avatarGradient(name),
        }}
      >
        {pending ? '?' : initials(name)}
        {!you && !pending && (
          <span
            style={{
              position: 'absolute',
              right: -1,
              bottom: -1,
              width: 9,
              height: 9,
              borderRadius: 999,
              background: online ? 'var(--green)' : 'var(--text-faint)',
              border: '2px solid var(--surface)',
            }}
          />
        )}
      </span>
      <span style={{ fontSize: 12.5, fontWeight: 600, color: pending ? 'var(--text-faint)' : 'var(--text)' }}>
        {you ? `${name} (you)` : name}
      </span>
      {onRemove && !you && (
        <button
          onClick={(e) => {
            e.stopPropagation()
            onRemove()
          }}
          title={pending ? 'Cancel this invite' : `Remove ${name} from this folder`}
          style={{
            display: 'grid',
            placeItems: 'center',
            width: 18,
            height: 18,
            borderRadius: 999,
            border: 'none',
            background: 'transparent',
            color: 'var(--text-faint)',
            cursor: 'pointer',
            padding: 0,
            flexShrink: 0,
          }}
          onMouseEnter={(e) => (e.currentTarget.style.color = 'var(--red)')}
          onMouseLeave={(e) => (e.currentTarget.style.color = 'var(--text-faint)')}
        >
          <X size={13} />
        </button>
      )}
    </div>
  )
}

function SettingRow({
  title,
  desc,
  children,
}: {
  title: string
  desc: string
  children: React.ReactNode
}) {
  return (
    <div style={{ display: 'flex', alignItems: 'center', gap: 14, padding: '11px 2px' }}>
      <div style={{ flex: 1 }}>
        <div style={{ fontSize: 13.5, fontWeight: 600 }}>{title}</div>
        <div style={{ fontSize: 12, color: 'var(--text-muted)', marginTop: 2, lineHeight: 1.45 }}>
          {desc}
        </div>
      </div>
      <div style={{ flexShrink: 0 }}>{children}</div>
    </div>
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
  const toast = useStore((s) => s.toast)
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    } catch {
      toast('error', 'Could not copy')
    }
  }
  return (
    <div
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
        initial={{ opacity: 0, scale: 0.96 }}
        animate={{ opacity: 1, scale: 1 }}
        onClick={(e) => e.stopPropagation()}
        className="card"
        style={{ width: 420, maxWidth: '100%', padding: 22, borderRadius: 20 }}
      >
        <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 14 }}>
          <div style={{ fontWeight: 750, fontSize: 16 }}>Invite for {folderName}</div>
          <button className="icon-btn" onClick={onClose}>
            <X size={17} />
          </button>
        </div>
        <p style={{ fontSize: 13, color: 'var(--text-muted)', marginTop: 0, lineHeight: 1.5 }}>
          Send this to the other person. They open DropBeam → <b>Accept invite</b>, paste it, and
          choose a folder.
        </p>
        <div style={{ display: 'grid', placeItems: 'center', margin: '6px 0 14px' }}>
          <div style={{ background: '#fff', padding: 12, borderRadius: 14, border: '1px solid var(--border)' }}>
            <QRCodeSVG value={code} size={150} level="M" fgColor="#15161d" bgColor="#fff" />
          </div>
        </div>
        <div
          className="selectable"
          style={{
            fontFamily: 'var(--font-mono)',
            fontSize: 11,
            background: 'var(--surface-2)',
            border: '1px solid var(--border)',
            borderRadius: 11,
            padding: '10px 12px',
            wordBreak: 'break-all',
            maxHeight: 80,
            overflowY: 'auto',
            color: 'var(--text-muted)',
          }}
        >
          {code}
        </div>
        <button className={`btn ${copied ? 'btn-ghost' : 'btn-primary'}`} style={{ width: '100%', marginTop: 12 }} onClick={copy}>
          {copied ? <Check size={15} /> : <Copy size={15} />}
          {copied ? 'Copied' : 'Copy invite'}
        </button>
      </motion.div>
    </div>
  )
}
