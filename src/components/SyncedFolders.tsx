import { useCallback, useEffect, useMemo, useState } from 'react'
import { Dialog } from './Dialog'
import { Dot, IconButton, MenuButton, SectionHeader, Spinner, type MenuItem } from './ui'
import { Check, Folder, FolderOpen, HardDrive, Pause, Play, RefreshCw, Trash2 } from 'lucide-react'
import {
  api, syncedFoldersApi, onSyncedFolderStatus,
  type SharedLocation, type SyncedFolder, type SyncedFolderStatus,
} from '../lib/api'
import { formatRelativeTime } from '../lib/format'
import { IS_MAC } from '../lib/platform'
import { destinationLabel, folderName, statusPill, type Pill } from '../lib/syncedFolders'
import { useStore } from '../store'

/** Friends' locations, as the Locations view already has them loaded. */
export type SharedByFriend = Record<string, SharedLocation[]>

/** Open the "Sync a folder" sheet from anywhere on the Locations page. */
const OPEN_EVENT = 'dropbeam:sync-folder'
// eslint-disable-next-line react-refresh/only-export-components -- tiny event helper shared with the page header
export function openSyncFolderSheet() { window.dispatchEvent(new CustomEvent(OPEN_EVENT)) }

const DOT: Record<Pill['tone'], 'ok' | 'off' | 'error' | 'busy'> = { ok: 'ok', busy: 'busy', wait: 'off', off: 'off', bad: 'error' }

/** "Up to date · checked 2m ago", "Paused", "Copying 12 files…" — one line of words. */
function statusWords(folder: SyncedFolder, status: SyncedFolderStatus | undefined, pill: Pill): string {
  if (pill.tone === 'off') return 'Paused'
  if (pill.tone === 'busy') return `${pill.label}…`
  const checked = status?.lastCheckAt || folder.lastCheckAt
  if (pill.tone === 'ok' && checked) return `${pill.label} · checked ${formatRelativeTime(checked).replace(/^Just now$/, 'just now')}`
  return pill.label
}

// ── The section ──────────────────────────────────────────────────────────────

/**
 * "Synced folders": the folders on THIS device that keep a friend's location
 * up to date. One way, on purpose — this is "my Mac feeds the NAS", not a
 * two-way mirror. One row per folder; everything but pause lives in its menu.
 */
export function SyncedFolders({ shared }: { shared: SharedByFriend }) {
  const friends = useStore(s => s.friends)
  const toast = useStore(s => s.toast)
  const [folders, setFolders] = useState<SyncedFolder[]>([])
  const [statuses, setStatuses] = useState<Record<string, SyncedFolderStatus>>({})
  const [sheet, setSheet] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState<SyncedFolder | null>(null)
  const [busy, setBusy] = useState('')

  const reload = useCallback(async () => {
    try {
      const [list, live] = await Promise.all([syncedFoldersApi.list(), syncedFoldersApi.statuses()])
      setFolders(list)
      setStatuses(prev => ({ ...prev, ...live }))
    } catch (e) { toast('error', String(e)) }
  }, [toast])

  useEffect(() => {
    void reload()
    const open = () => setSheet(true)
    window.addEventListener(OPEN_EVENT, open)
    let un: (() => void) | undefined
    let alive = true
    void onSyncedFolderStatus(s => setStatuses(prev => ({ ...prev, [s.id]: s })))
      .then(fn => { if (alive) un = fn; else fn() })
    return () => { alive = false; un?.(); window.removeEventListener(OPEN_EVENT, open) }
  }, [reload])

  const act = async (id: string, run: () => Promise<unknown>) => {
    setBusy(id)
    try { await run(); await reload() } catch (e) { toast('error', String(e)) } finally { setBusy('') }
  }

  const place = (folder: SyncedFolder) => {
    const friend = friends.find(f => f.id === folder.friendId)
    const location = shared[folder.friendId]?.find(l => l.id === folder.locationId)
    // The location's own name is only known while its host is reachable; until
    // then say whose folder it is rather than something anonymous.
    const name = location?.name || (friend ? `${friend.name}’s folder` : 'a removed device')
    return { location: name, label: destinationLabel(name, folder.relPath) }
  }

  const removing = confirmRemove ? place(confirmRemove) : null
  return <>
    {folders.length > 0 && <section className="location-section" aria-label="Synced folders">
      <SectionHeader>Synced folders</SectionHeader>
      <div className="group">{folders.map(folder => {
        const status = statuses[folder.id]
        const pill = statusPill(folder, status)
        const where = place(folder)
        const words = statusWords(folder, status, pill)
        const working = busy === folder.id
        const items: MenuItem[] = [
          { label: 'Sync now', icon: <RefreshCw />, disabled: working || !folder.enabled, onSelect: () => void act(folder.id, () => syncedFoldersApi.syncNow(folder.id)) },
          { label: IS_MAC ? 'Open in Finder' : 'Open folder', icon: <FolderOpen />, onSelect: () => { void api.openPath(folder.localPath) } },
          { separator: true },
          { heading: 'When you delete a file here' },
          { label: `Keep the copy on ${where.location}`, icon: folder.deleteRemote ? <span className="location-menu-space" /> : <Check />, disabled: working,
            onSelect: () => { if (folder.deleteRemote) void act(folder.id, () => syncedFoldersApi.update(folder.id, { deleteRemote: false })) } },
          { label: 'Move the copy to Trash too', icon: folder.deleteRemote ? <Check /> : <span className="location-menu-space" />, disabled: working,
            onSelect: () => { if (!folder.deleteRemote) void act(folder.id, () => syncedFoldersApi.update(folder.id, { deleteRemote: true })) } },
          { separator: true },
          { label: 'Stop syncing…', icon: <Trash2 />, danger: true, disabled: working, onSelect: () => setConfirmRemove(folder) },
        ]
        return <div className="row location-row" key={folder.id}>
          <span className="location-glyph" aria-hidden><Folder /></span>
          <div className="row-main">
            <div className="row-title truncate-1" title={folder.localPath}>{folderName(folder.localPath)}</div>
            <div className="row-sub truncate-1" title={where.label}>To {where.label}</div>
          </div>
          <div className="row-trailing">
            <span className={`location-status tone-${pill.tone}`} title={words} role="status">
              {pill.tone === 'busy' ? <Spinner size={10} /> : <Dot tone={DOT[pill.tone]} />}
              <span className="truncate-1">{words}</span>
            </span>
            <IconButton label={folder.enabled ? `Pause ${folderName(folder.localPath)}` : `Resume ${folderName(folder.localPath)}`}
              tooltip={folder.enabled ? 'Pause' : 'Resume'} disabled={working}
              onClick={() => void act(folder.id, () => syncedFoldersApi.update(folder.id, { enabled: !folder.enabled }))}>
              {folder.enabled ? <Pause fill="currentColor" strokeWidth={0} /> : <Play />}
            </IconButton>
            <MenuButton label={`More for ${folderName(folder.localPath)}`} items={items} />
          </div>
        </div>
      })}</div>
    </section>}

    {confirmRemove && removing && <Dialog title={`Stop syncing “${folderName(confirmRemove.localPath)}”?`} width={380}
      busy={busy === confirmRemove.id} onClose={() => setConfirmRemove(null)}
      footer={<>
        <button className="btn btn-secondary" disabled={busy === confirmRemove.id} onClick={() => setConfirmRemove(null)}>Cancel</button>
        <button className="btn btn-destructive" disabled={busy === confirmRemove.id}
          onClick={() => { const f = confirmRemove; void act(f.id, () => syncedFoldersApi.remove(f.id)).then(() => setConfirmRemove(null)) }}>Stop syncing</button>
      </>}>
      <p className="dialog-text">Nothing is deleted. The folder stays on this device and the copies stay on {removing.location}.</p>
    </Dialog>}

    {sheet && <SyncSheet shared={shared} onClose={() => setSheet(false)} onDone={() => { setSheet(false); void reload() }} />}
  </>
}

// ── The sheet ────────────────────────────────────────────────────────────────

type Choice = { friendId: string; locationId: string; name: string }

function SyncSheet({ shared, onClose, onDone }: { shared: SharedByFriend; onClose: () => void; onDone: () => void }) {
  const friends = useStore(s => s.friends)
  const toast = useStore(s => s.toast)
  const [localPath, setLocalPath] = useState('')
  const [choice, setChoice] = useState<Choice | null>(null)
  const [relPath, setRelPath] = useState('')
  const [deleteRemote, setDeleteRemote] = useState(false)
  const [busy, setBusy] = useState(false)

  // Only folders a friend actually lets this device add files to.
  const options = useMemo(() => friends.flatMap(f =>
    (shared[f.id] || []).filter(l => l.rights.upload).map(l => ({ friendId: f.id, locationId: l.id, name: l.name, friend: f.name }))
  ), [friends, shared])
  // One destination? It's the answer — don't make anyone click it.
  const picked = choice ?? (options.length === 1 ? { friendId: options[0].friendId, locationId: options[0].locationId, name: options[0].name } : null)

  const pick = async () => {
    try {
      const dir = await api.pickDirectory()
      if (!dir) return
      setLocalPath(dir)
      // Default the destination folder to the folder's own name, so the NAS
      // gets "Travel", not a pile of loose files at its top level.
      setRelPath(folderName(dir))
    } catch (e) { toast('error', String(e)) }
  }

  const save = async () => {
    if (!localPath || !picked) return
    setBusy(true)
    try {
      await syncedFoldersApi.add(picked.friendId, picked.locationId, relPath.trim(), localPath, deleteRemote)
      toast('success', `“${folderName(localPath)}” will be kept copied to ${picked.name}.`)
      onDone()
    } catch (e) { toast('error', String(e)); setBusy(false) }
  }

  const footer = <>
    <button className="btn btn-secondary" disabled={busy} onClick={onClose}>Cancel</button>
    <button className="btn btn-primary" disabled={busy || !picked || !localPath} onClick={() => void save()}>
      {busy ? 'Starting…' : 'Start syncing'}</button>
  </>
  return <Dialog title="Sync a folder" width={440} onClose={onClose} busy={busy} className="location-dialog" footer={footer}>
    <div className="location-form">
      <div className="location-field">
        <span className="field-label">Folder on this device</span>
        <div className="location-choose">
          {localPath
            ? <span className="location-choose-name" title={localPath}><Folder /><span className="truncate-1">{folderName(localPath)}</span></span>
            : <span className="location-choose-name faint">No folder chosen</span>}
          <button type="button" className="btn btn-secondary" onClick={() => void pick()}>{localPath ? 'Change…' : 'Choose…'}</button>
        </div>
      </div>

      <div className="location-field">
        <span className="field-label" id="sync-dest-label">Copy to</span>
        {!options.length
          ? <p className="field-hint">None of your friends’ locations accept files from you yet.</p>
          : <div className="group location-pick-group" role="radiogroup" aria-labelledby="sync-dest-label">{options.map(o => {
            const on = picked?.locationId === o.locationId && picked?.friendId === o.friendId
            return <button key={`${o.friendId}:${o.locationId}`} type="button" role="radio" aria-checked={on}
              className={`row location-pick${on ? ' on' : ''}`}
              onClick={() => setChoice({ friendId: o.friendId, locationId: o.locationId, name: o.name })}>
              <span className="location-glyph" aria-hidden><HardDrive /></span>
              <span className="row-main"><span className="row-title truncate-1">{o.name}</span><span className="row-sub truncate-1">{o.friend}</span></span>
              {on && <Check className="location-pick-check" aria-hidden />}
            </button>
          })}</div>}
      </div>

      <label className="location-field">
        <span className="field-label">Into folder <span className="optional">(optional)</span></span>
        <input className="input" value={relPath} onChange={e => setRelPath(e.target.value)} placeholder="Top level" />
      </label>

      <label className="location-check">
        <input type="checkbox" checked={deleteRemote} onChange={e => setDeleteRemote(e.target.checked)} />
        <span>When I delete a file here, move its copy to {picked?.name ? `${picked.name}’s` : 'the location’s'} Trash</span>
      </label>
    </div>
  </Dialog>
}
