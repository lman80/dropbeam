import { useCallback, useEffect, useMemo, useState } from 'react'
import {
  AlertTriangle, ArrowRight, Check, Clock, FolderOpen, FolderSync, HardDrive,
  Pause, Play, Plus, RefreshCw, Trash2, Upload, X,
} from 'lucide-react'
import {
  api, syncedFoldersApi, onSyncedFolderStatus,
  type SharedLocation, type SyncedFolder, type SyncedFolderStatus,
} from '../lib/api'
import { formatRelativeTime } from '../lib/format'
import { destinationLabel, folderName, statusPill, summarySentence } from '../lib/syncedFolders'
import { useStore } from '../store'
import './locations.css'

/** Friends' locations, as the Locations view already has them loaded. */
export type SharedByFriend = Record<string, SharedLocation[]>

/** Open the "Sync a folder here" sheet from anywhere on the Locations page. */
const OPEN_EVENT = 'dropbeam:sync-folder'
export function openSyncFolderSheet() { window.dispatchEvent(new CustomEvent(OPEN_EVENT)) }

/**
 * The button that starts the whole thing, sitting above the friends' location
 * cards so "I want a folder that just goes to the NAS" is one click from the
 * place those folders live.
 */
export function SyncFolderToolbar() {
  return <div className="location-sync-cta">
    <div><strong>Keep a folder on this device copied to one of these</strong>
      <p className="location-muted">Drop files into a folder here and they turn up there — no dragging, no thinking about it.</p></div>
    <button className="btn btn-ghost" onClick={openSyncFolderSheet}><FolderSync size={15} /> Sync a folder here</button>
  </div>
}

// ── The section ──────────────────────────────────────────────────────────────

/**
 * "Synced to a location": the folders on THIS device that keep a friend's
 * location up to date. One way, on purpose — this is "my Mac feeds the NAS",
 * not a two-way mirror.
 */
export function SyncedFolders({ shared }: { shared: SharedByFriend }) {
  const friends = useStore(s => s.friends)
  const toast = useStore(s => s.toast)
  const [folders, setFolders] = useState<SyncedFolder[]>([])
  const [statuses, setStatuses] = useState<Record<string, SyncedFolderStatus>>({})
  const [sheet, setSheet] = useState(false)
  const [confirmRemove, setConfirmRemove] = useState('')
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
    const name = location?.name || (friend ? `${friend.name}’s folder` : 'that folder')
    return { friend: friend?.name || 'a device you removed', location: name, label: destinationLabel(name, folder.relPath) }
  }

  if (!folders.length && !sheet) return null
  return <section className="location-synced" aria-label="Folders synced to a location">
    <div className="location-heading"><div><h2><FolderSync size={17} /> Synced to a location</h2>
      <p>Folders on this device that copy themselves to a friend’s folder. Anything you put in one turns up there.</p></div>
      <button className="btn btn-ghost" onClick={() => setSheet(true)}><Plus size={15} /> Sync a folder</button></div>

    <div className="location-grid">{folders.map(folder => {
      const status = statuses[folder.id]
      const pill = statusPill(folder, status)
      const where = place(folder)
      const checked = status?.lastCheckAt || folder.lastCheckAt
      return <div className="card location-tile location-synced-tile" key={folder.id}>
        <div className="location-tile-top"><span className="location-drive-icon"><FolderSync size={22} /></span>
          <span className={`location-pill ${pill.tone}`}>
            {pill.tone === 'ok' && <Check size={11} />}
            {pill.tone === 'busy' && <Upload size={11} />}
            {pill.tone === 'wait' && <Clock size={11} />}
            {pill.tone === 'bad' && <AlertTriangle size={11} />}
            {pill.tone === 'off' && <Pause size={11} />}
            {pill.label}</span></div>
        <h2>{folderName(folder.localPath)}</h2>
        <div className="location-route"><span title={folder.localPath}>This device</span><ArrowRight size={13} />
          <span title={where.label}><HardDrive size={12} /> {where.label}</span></div>
        <p className="location-path" title={folder.localPath}>{folder.localPath}</p>
        <small className="location-muted location-last">{checked
          ? <>Last checked {formatRelativeTime(checked)}</>
          : <>Not checked yet</>}{folder.deleteRemote
            ? ' · deletes are copied over too'
            : ` · files you delete here stay on ${where.location}`}</small>
        <div className="location-tile-actions">
          <button className="btn btn-ghost" disabled={busy === folder.id || !folder.enabled}
            onClick={() => void act(folder.id, () => syncedFoldersApi.syncNow(folder.id))}>
            <RefreshCw size={14} className={status?.state === 'scanning' || status?.state === 'uploading' ? 'location-spin' : ''} /> Sync now</button>
          <button className="btn btn-ghost" disabled={busy === folder.id}
            onClick={() => void act(folder.id, () => syncedFoldersApi.update(folder.id, { enabled: !folder.enabled }))}>
            {folder.enabled ? <><Pause size={14} /> Pause</> : <><Play size={14} /> Resume</>}</button>
          <button className="btn btn-ghost" onClick={() => { void api.openPath(folder.localPath) }}>
            <FolderOpen size={14} /> Open folder</button>
          {confirmRemove === folder.id
            ? <>
              <button className="btn btn-danger" disabled={busy === folder.id}
                onClick={() => { setConfirmRemove(''); void act(folder.id, () => syncedFoldersApi.remove(folder.id)) }}>
                Stop copying</button>
              <button className="btn btn-ghost" onClick={() => setConfirmRemove('')}>Keep it</button>
            </>
            : <button className="btn btn-ghost" onClick={() => setConfirmRemove(folder.id)}><Trash2 size={14} /> Remove</button>}
        </div>
        {confirmRemove === folder.id && <p className="location-muted" role="status">
          Nothing is deleted — the folder stays on this device and the copies stay on {where.location}.</p>}
      </div>
    })}</div>

    {sheet && <SyncSheet shared={shared} onClose={() => setSheet(false)} onDone={() => { setSheet(false); void reload() }} />}
  </section>
}

// ── The 3-step sheet ─────────────────────────────────────────────────────────

type Choice = { friendId: string; locationId: string; name: string }

function SyncSheet({ shared, onClose, onDone }: { shared: SharedByFriend; onClose: () => void; onDone: () => void }) {
  const friends = useStore(s => s.friends)
  const toast = useStore(s => s.toast)
  const [step, setStep] = useState(1)
  const [localPath, setLocalPath] = useState('')
  const [choice, setChoice] = useState<Choice | null>(null)
  const [relPath, setRelPath] = useState('')
  const [deleteRemote, setDeleteRemote] = useState(false)
  const [advanced, setAdvanced] = useState(false)
  const [busy, setBusy] = useState(false)

  // Only folders a friend actually lets this device add files to.
  const options = useMemo(() => friends.flatMap(f =>
    (shared[f.id] || []).filter(l => l.rights.upload).map(l => ({ friendId: f.id, locationId: l.id, name: l.name, friend: f.name }))
  ), [friends, shared])

  const pick = async () => {
    try {
      const dir = await api.pickDirectory()
      if (!dir) return
      setLocalPath(dir)
      // Default the destination folder to the folder's own name, so the NAS
      // gets "Travel", not a pile of loose files at its top level.
      setRelPath(folderName(dir))
      setStep(2)
    } catch (e) { toast('error', String(e)) }
  }

  const save = async () => {
    if (!localPath || !choice) return
    setBusy(true)
    try {
      await syncedFoldersApi.add(choice.friendId, choice.locationId, relPath.trim(), localPath, deleteRemote)
      toast('success', `“${folderName(localPath)}” will be kept copied to ${choice.name}.`)
      onDone()
    } catch (e) { toast('error', String(e)); setBusy(false) }
  }

  return <div className="location-modal location-sync-sheet" role="dialog" aria-modal="true" aria-label="Sync a folder to a location"
    onClick={e => { if (e.target === e.currentTarget) onClose() }}>
    <div className="card">
      <div className="location-sheet-head">
        <h2><FolderSync size={18} /> Sync a folder</h2>
        <button className="btn btn-ghost" onClick={onClose} aria-label="Close"><X size={16} /></button>
      </div>
      <ol className="location-steps">
        {['Choose the folder', 'Choose where it goes', 'Check it over'].map((label, i) => (
          <li key={label} className={step === i + 1 ? 'now' : step > i + 1 ? 'done' : ''}>
            <i>{step > i + 1 ? <Check size={11} /> : i + 1}</i>{label}</li>
        ))}
      </ol>

      {step === 1 && <div className="location-step">
        <p>Pick a folder on this device. Everything already in it, and everything you add later, gets copied over.</p>
        <button className="btn btn-primary" onClick={() => void pick()}><FolderOpen size={15} /> Choose a folder…</button>
        {localPath && <p className="location-path">{localPath}</p>}
      </div>}

      {step === 2 && <div className="location-step">
        <p>Where should the copies go?</p>
        {!options.length && <p className="location-muted">
          None of your friends’ folders accept files from this device yet. Open one of their locations first, or ask them to allow uploads.</p>}
        <div className="location-pick-list">{options.map(o => (
          <button key={`${o.friendId}:${o.locationId}`} type="button"
            className={`location-pick ${choice?.locationId === o.locationId && choice?.friendId === o.friendId ? 'on' : ''}`}
            onClick={() => setChoice({ friendId: o.friendId, locationId: o.locationId, name: o.name })}>
            <HardDrive size={17} /><span><strong>{o.name}</strong><small>{o.friend}</small></span>
            {choice?.locationId === o.locationId && choice?.friendId === o.friendId && <Check size={15} />}
          </button>
        ))}</div>
        <label>Folder to put them in
          <input value={relPath} onChange={e => setRelPath(e.target.value)} placeholder="Leave empty for the top level" />
        </label>
        <div className="dialog-actions">
          <button className="btn btn-ghost" onClick={() => setStep(1)}>Back</button>
          <button className="btn btn-primary" disabled={!choice} onClick={() => setStep(3)}>Next<ArrowRight size={15} /></button>
        </div>
      </div>}

      {step === 3 && choice && <div className="location-step">
        <p className="location-summary">{summarySentence(localPath, choice.name, relPath, deleteRemote)}</p>
        <p className="location-muted">It checks for new files the moment you add them, and again every half hour just in case.</p>
        <details open={advanced} onToggle={e => setAdvanced((e.target as HTMLDetailsElement).open)}>
          <summary>Advanced</summary>
          <label className="location-check">
            <input type="checkbox" checked={deleteRemote} onChange={e => setDeleteRemote(e.target.checked)} />
            <span>Also remove the copy when I delete a file here
              <small className="location-muted"> — the copy goes to {choice.name}’s trash, where it can still be recovered.</small></span>
          </label>
        </details>
        <div className="dialog-actions">
          <button className="btn btn-ghost" onClick={() => setStep(2)}>Back</button>
          <button className="btn btn-primary" disabled={busy} onClick={() => void save()}>
            {busy ? 'Setting it up…' : 'Start syncing'}</button>
        </div>
      </div>}
    </div>
  </div>
}
