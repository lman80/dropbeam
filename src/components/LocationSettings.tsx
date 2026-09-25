/* eslint-disable react-refresh/only-export-components -- peopleLabel/accessLabel are shared with the Locations page */
import { useEffect, useState } from 'react'
import { Check, FolderOpen, HardDrive, Pencil, Server, Trash2 } from 'lucide-react'
import { api, locationsApi, onLocationActivity, type LocationActivity, type HostedLocation, type LocationRights, type MountCandidate } from '../lib/api'
import { formatBytes, formatRelativeTime } from '../lib/format'
import { IS_MAC, IS_WINDOWS } from '../lib/platform'
import { useStore } from '../store'
import { Dialog } from './Dialog'
import { MenuButton, SectionHeader } from './ui'

const DEFAULT_CAP = 500_000_000_000
const empty = (): HostedLocation => ({ id: '', name: '', path: '', friendIds: [], rights: { upload: true, manage: false }, byteCap: DEFAULT_CAP })
/** The folder's own name, so "where is it?" already answers "what's it called?". */
const nameFromPath = (path: string): string => path.split('/').filter(Boolean).pop() || 'Shared folder'
const baseName = (path: string): string => path.split('/').filter(Boolean).pop() || path
/** What a friend may do, as plain-language choices instead of two checkboxes. */
const ACCESS = [
  { id: 'read', label: 'View and download', hint: 'They can open it and copy things out.', rights: { upload: false, manage: false } },
  { id: 'add', label: 'Add files', hint: 'They can also put files in. Nothing is overwritten.', rights: { upload: true, manage: false } },
  { id: 'manage', label: 'Add, rename and delete', hint: 'Deleted items go to the folder’s Trash.', rights: { upload: true, manage: true } },
] as const
const accessOf = (rights: { upload: boolean; manage: boolean }): string =>
  rights.manage ? 'manage' : rights.upload ? 'add' : 'read'

/** "Alex", "Alex and Chen Wei", "Alex, Chen Wei and 1 more". */
export function peopleLabel(names: string[]): string {
  if (!names.length) return 'Only you'
  if (names.length <= 2) return names.join(' and ')
  return `${names[0]}, ${names[1]} and ${names.length - 2} more`
}
/** The rights a location hands out, in three words or fewer. */
export function accessLabel(rights: LocationRights): string {
  return rights.manage ? (rights.upload ? 'Full access' : 'Can rename and delete') : rights.upload ? 'Can add files' : 'View only'
}

/** "Alex uploaded W-2.pdf" — one activity entry in words. */
function activitySentence(a: LocationActivity, who: string): string {
  const item = baseName(a.item)
  switch (a.operation.replace(/^locations\./, '')) {
    case 'upload': return `${who} uploaded ${item}`
    case 'download': return `${who} downloaded ${item}`
    case 'mkdir': return `${who} created the folder ${item}`
    case 'rename': return a.to ? `${who} renamed ${item} to ${baseName(a.to)}` : `${who} renamed ${item}`
    case 'trash': return `${who} moved ${item} to Trash`
    default: return `${who} changed ${item}`
  }
}

/** Friend chips: tap to give or take away access. */
function PeoplePicker({ ids, onChange }: { ids: string[]; onChange: (ids: string[]) => void }) {
  const friends = useStore(s => s.friends)
  if (!friends.length) return <p className="field-hint">No friends yet. You can share it now and pick people later.</p>
  return <div className="location-chips" role="group" aria-label="People">{friends.map(f => {
    const on = ids.includes(f.id)
    return <button type="button" key={f.id} className={`pick-chip${on ? ' on' : ''}`} aria-pressed={on} title={f.name}
      onClick={() => onChange(on ? ids.filter(id => id !== f.id) : [...ids, f.id])}>
      {on && <Check size={12} aria-hidden />}<span className="truncate-1">{f.name}</span></button>
  })}</div>
}

const capGb = (l: HostedLocation) => (l.byteCap ?? DEFAULT_CAP) / 1_000_000_000

/**
 * Adding a folder asks the three questions a person actually has, one at a
 * time, and offers the NAS shares and drives this device can already see as
 * one-click answers.
 */
function AddLocationWizard({ onCancel, onSaved }: { onCancel: () => void; onSaved: (locations: HostedLocation[], addedId?: string) => void }) {
  const [draft, setDraft] = useState<HostedLocation>(empty)
  const [step, setStep] = useState<'where' | 'name' | 'who'>('where')
  const [mounts, setMounts] = useState<MountCandidate[] | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  useEffect(() => {
    let alive = true
    locationsApi.mountCandidates().then(v => { if (alive) setMounts(v) }).catch(() => { if (alive) setMounts([]) })
    return () => { alive = false }
  }, [])
  const choose = (path: string, label?: string) => {
    setError('')
    setDraft(d => ({ ...d, path, name: d.name || label || nameFromPath(path) }))
    setStep('name')
  }
  const browse = async () => {
    try { const path = await api.pickDirectory(); if (path) choose(path) }
    catch (e) { setError(String(e)) }
  }
  const save = async () => {
    if (busy) return
    setBusy(true); setError('')
    try {
      const before = new Set((await locationsApi.listHosted()).map(l => l.id))
      const locations = await locationsApi.save({ ...draft, name: draft.name.trim() })
      onSaved(locations, locations.find(l => !before.has(l.id))?.id)
    } catch (e) { setError(String(e)) }
    finally { setBusy(false) }
  }
  const access = accessOf(draft.rights)
  const submit = () => {
    if (step === 'name') { if (draft.name.trim()) setStep('who'); return }
    if (step === 'who') void save()
  }
  const title = step === 'where' ? 'Choose a folder to share' : step === 'name' ? 'Name this location' : 'Choose who can use it'
  const footer = step === 'where'
    ? <button type="button" className="btn btn-secondary" onClick={onCancel}>Cancel</button>
    : <>
      <button type="button" className="btn btn-secondary" disabled={busy} onClick={() => setStep(step === 'who' ? 'name' : 'where')}>Back</button>
      <button type="submit" form="location-wizard" className="btn btn-primary"
        disabled={busy || !draft.name.trim() || (step === 'who' && !draft.path.trim())}>
        {step === 'name' ? 'Next' : busy ? 'Sharing…' : 'Share'}</button>
    </>
  return <Dialog title={title} width={440} busy={busy} onClose={onCancel} className="location-dialog" footer={footer}>
    <form id="location-wizard" className="location-form" onSubmit={e => { e.preventDefault(); submit() }}>
      {step === 'where' && <div className="group location-pick-group">
        {mounts === null && <div className="row"><span className="row-sub">Looking for drives…</span></div>}
        {(mounts || []).map(m => <button type="button" className="row location-pick" key={m.path} title={m.path} onClick={() => choose(m.path, m.label)}>
          <span className="location-glyph" aria-hidden>{m.kind === 'network' ? <Server /> : <HardDrive />}</span>
          <span className="row-main">
            <span className="row-title truncate-1">{m.label}</span>
            <span className="row-sub truncate-1">{m.kind === 'network' ? 'Network drive' : 'External disk'}{m.freeBytes != null && m.totalBytes ? ` · ${formatBytes(m.freeBytes)} free` : ''}</span>
          </span>
        </button>)}
        <button type="button" className="row location-pick" onClick={() => { void browse() }}>
          <span className="location-glyph" aria-hidden><FolderOpen /></span>
          <span className="row-main"><span className="row-title">Choose another folder…</span></span>
        </button>
      </div>}

      {step === 'name' && <>
        <label className="location-field">
          <span className="field-label">Name</span>
          <input className="input" autoFocus required maxLength={80} placeholder="Family NAS" value={draft.name}
            onChange={e => setDraft({ ...draft, name: e.target.value })} />
          <span className="field-hint">Friends see this name. The folder itself isn’t renamed.</span>
        </label>
        <div className="location-field">
          <span className="field-label">Folder</span>
          <span className="location-choose-name" title={draft.path}><FolderOpen /><span className="truncate-1">{nameFromPath(draft.path)}</span></span>
        </div>
      </>}

      {step === 'who' && <>
        <div className="location-field">
          <span className="field-label">People</span>
          <PeoplePicker ids={draft.friendIds} onChange={friendIds => setDraft({ ...draft, friendIds })} />
        </div>
        <div className="location-field" role="radiogroup" aria-label="What they can do">
          <span className="field-label">What they can do</span>
          <div className="location-options">{ACCESS.map(option => <button type="button" key={option.id} role="radio" aria-checked={access === option.id}
            className={`option-card${access === option.id ? ' on' : ''}`} onClick={() => setDraft({ ...draft, rights: { ...option.rights } })}>
            <span className="option-card-title">{option.label}</span>
            <span className="option-card-desc">{option.hint}</span>
          </button>)}</div>
        </div>
        <details className="location-advanced">
          <summary>Advanced</summary>
          <label className="location-field location-cap">
            <span className="field-label">Largest single transfer</span>
            <span className="location-cap-input"><input className="input tnum" type="number" min="0.001" step="0.001" required value={capGb(draft)}
              onChange={e => setDraft({ ...draft, byteCap: Math.max(1, Math.round(Number(e.target.value) * 1_000_000_000)) })} /><span className="muted">GB</span></span>
          </label>
        </details>
      </>}
      {error && <p className="form-error" role="alert">{error}</p>}
    </form>
  </Dialog>
}

function EditLocation({ draft: initial, onClose, onSaved }: { draft: HostedLocation; onClose: () => void; onSaved: (locations: HostedLocation[]) => void }) {
  const [draft, setDraft] = useState(initial)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const save = async () => {
    if (busy) return
    setBusy(true); setError('')
    try { onSaved(await locationsApi.save(draft)) }
    catch (e) { setError(String(e)) }
    finally { setBusy(false) }
  }
  const footer = <>
    <button type="button" className="btn btn-secondary" disabled={busy} onClick={onClose}>Cancel</button>
    <button type="submit" form="location-edit" className="btn btn-primary" disabled={busy || !draft.name.trim() || !draft.path.trim()}>{busy ? 'Saving…' : 'Save'}</button>
  </>
  const right = (key: 'upload' | 'manage', label: string) => <label className="location-check">
    <input type="checkbox" checked={draft.rights[key]} onChange={e => setDraft({ ...draft, rights: { ...draft.rights, [key]: e.target.checked } })} />
    <span>{label}</span></label>
  return <Dialog title={`Edit “${initial.name}”`} width={440} busy={busy} onClose={onClose} className="location-dialog" footer={footer}>
    <form id="location-edit" className="location-form" onSubmit={e => { e.preventDefault(); void save() }}>
      <label className="location-field">
        <span className="field-label">Name</span>
        <input className="input" autoFocus required maxLength={80} placeholder="Family NAS" value={draft.name} onChange={e => setDraft({ ...draft, name: e.target.value })} />
      </label>
      <label className="location-field">
        <span className="field-label">Folder</span>
        <span className="location-inline">
          <input className="input" required placeholder="/Volumes/NAS/Shared" value={draft.path} title={draft.path} onChange={e => setDraft({ ...draft, path: e.target.value })} />
          <button type="button" className="btn btn-secondary" onClick={async () => {
            try { const path = await api.pickDirectory(); if (path) setDraft(d => ({ ...d, path, name: d.name || nameFromPath(path) })) } catch (e) { setError(String(e)) }
          }}>Choose…</button>
        </span>
      </label>
      <div className="location-field">
        <span className="field-label">People</span>
        <PeoplePicker ids={draft.friendIds} onChange={friendIds => setDraft({ ...draft, friendIds })} />
      </div>
      <div className="location-field">
        <span className="field-label">They can also</span>
        {right('upload', 'Add files and folders')}
        {right('manage', 'Create folders, rename and move to Trash')}
        <span className="field-hint">Everyone you pick can view and download.</span>
      </div>
      <label className="location-field location-cap">
        <span className="field-label">Largest single transfer</span>
        <span className="location-cap-input"><input className="input tnum" type="number" min="0.001" step="0.001" required value={capGb(draft)}
          onChange={e => setDraft({ ...draft, byteCap: Math.round(Number(e.target.value) * 1_000_000_000) })} /><span className="muted">GB</span></span>
      </label>
      {error && <p className="form-error" role="alert">{error}</p>}
    </form>
  </Dialog>
}

const ACTIVITY_CAP = 5

export function LocationSettings() {
  const friends = useStore(s => s.friends)
  const [locations, setLocations] = useState<HostedLocation[]>([])
  const [draft, setDraft] = useState<HostedLocation | null>(null)
  const [adding, setAdding] = useState(false)
  const [added, setAdded] = useState('')
  const [removing, setRemoving] = useState<HostedLocation | null>(null)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [activity, setActivity] = useState<LocationActivity[]>([])
  const [allActivity, setAllActivity] = useState(false)
  const [loaded, setLoaded] = useState(false)
  useEffect(() => {
    let alive = true
    locationsApi.listHosted().then(v => { if (alive) { setLocations(v); setLoaded(true) } }).catch(e => { if (alive) setError(String(e)) })
    return () => { alive = false }
  }, [])
  useEffect(() => {
    let alive = true; let un: (() => void) | undefined
    void locationsApi.activity().then(rows => { if (alive) setActivity(existing => [...existing, ...rows].filter((a, i, all) => all.findIndex(b => b.at === a.at && b.item === a.item && b.friendId === a.friendId && b.operation === a.operation) === i).slice(0, 50)) }).catch(() => {})
    void onLocationActivity(a => setActivity(rows => [a, ...rows].slice(0, 50))).then(fn => { if (alive) un = fn; else fn() })
    return () => { alive = false; un?.() }
  }, [])
  const startAdding = () => { setError(''); setAdded(''); setDraft(null); setAdding(true) }
  const stopSharing = async (l: HostedLocation) => {
    setBusy(true); setError('')
    try { setLocations(await locationsApi.remove(l.id)); if (draft?.id === l.id) setDraft(null); setRemoving(null) }
    catch (e) { setError(String(e)); setRemoving(null) } finally { setBusy(false) }
  }
  const nameOf = (id: string) => friends.find(f => f.id === id)?.name
  const shownActivity = allActivity ? activity : activity.slice(0, ACTIVITY_CAP)

  return <section className="location-settings" aria-label="Locations">
    <SectionHeader action={!IS_WINDOWS && <button className="btn btn-secondary btn-sm" disabled={!loaded || busy || adding} onClick={startAdding}>Share a folder…</button>}>
      Locations</SectionHeader>
    {error && <p className="form-error location-settings-error" role="alert">{error}</p>}
    <div className="group">
      {!loaded && !error && <div className="row"><span className="row-sub">Loading…</span></div>}
      {IS_WINDOWS && loaded && !locations.length && <div className="row"><span className="row-sub">Sharing a folder from Windows isn’t available yet. You can still use locations friends share with you.</span></div>}
      {!IS_WINDOWS && loaded && !locations.length && <div className="row"><span className="row-sub">Share a folder or NAS with the friends you pick.</span></div>}
      {locations.map(l => {
        const members = l.friendIds.map(id => nameOf(id) || 'a removed device')
        return <div className="row location-row" key={l.id}>
          <span className="location-glyph" aria-hidden><HardDrive /></span>
          <div className="row-main">
            <div className="row-title truncate-1" title={l.path}>{l.name}</div>
            {l.id === added
              ? <div className="row-sub location-added truncate-1"><Check size={12} aria-hidden /> Shared · friends find it under Locations</div>
              : <div className="row-sub truncate-1" title={members.join(', ')}>{members.length ? `${peopleLabel(members)} · ${accessLabel(l.rights)}` : 'Only you'}</div>}
          </div>
          <div className="row-trailing">
            <MenuButton label={`More for ${l.name}`} items={[
              { label: 'Edit…', icon: <Pencil />, disabled: busy, onSelect: () => { setError(''); setAdding(false); setDraft({ ...l, friendIds: [...l.friendIds], rights: { ...l.rights } }) } },
              { label: IS_MAC ? 'Open in Finder' : 'Open folder', icon: <FolderOpen />, onSelect: () => { void api.openPath(l.path).catch(e => setError(String(e))) } },
              { separator: true },
              { label: 'Stop sharing…', icon: <Trash2 />, danger: true, disabled: busy, onSelect: () => setRemoving(l) },
            ]} />
          </div>
        </div>
      })}
    </div>

    {activity.length > 0 && <>
      <SectionHeader action={activity.length > ACTIVITY_CAP && <button className="btn btn-plain btn-sm" onClick={() => setAllActivity(v => !v)}>{allActivity ? 'Show less' : 'Show all'}</button>}>
        Recent activity</SectionHeader>
      <div className="group">{shownActivity.map((a, i) => {
        const where = locations.length > 1 ? locations.find(l => l.id === a.locationId)?.name : undefined
        const sentence = activitySentence(a, nameOf(a.friendId) || 'Someone')
        return <div className="row location-activity-row" key={`${a.at}:${i}`}>
          <span className="row-main truncate-1" title={a.to ? `${a.item} → ${a.to}` : a.item}>{sentence}{where ? <span className="muted"> in {where}</span> : null}</span>
          <span className="row-trailing muted tnum">{formatRelativeTime(a.at)}</span>
        </div>
      })}</div>
    </>}

    {adding && <AddLocationWizard onCancel={() => setAdding(false)}
      onSaved={(list, id) => { setLocations(list); setAdding(false); setAdded(id || '') }} />}
    {draft && <EditLocation draft={draft} onClose={() => setDraft(null)} onSaved={list => { setLocations(list); setDraft(null) }} />}
    {removing && <Dialog title={`Stop sharing “${removing.name}”?`} width={380} busy={busy} onClose={() => setRemoving(null)}
      footer={<>
        <button className="btn btn-secondary" disabled={busy} onClick={() => setRemoving(null)}>Cancel</button>
        <button className="btn btn-destructive" disabled={busy} onClick={() => void stopSharing(removing)}>Stop sharing</button>
      </>}>
      <p className="dialog-text">Friends lose access. Nothing in the folder is deleted.</p>
    </Dialog>}
  </section>
}
