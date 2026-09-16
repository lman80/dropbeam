import { useEffect, useState } from 'react'
import { Check, ChevronLeft, ChevronRight, FolderOpen, HardDrive, Plus, Server, X } from 'lucide-react'
import { api, locationsApi, onLocationActivity, type LocationActivity, type HostedLocation, type MountCandidate } from '../lib/api'
import { formatBytes } from '../lib/format'
import { useStore } from '../store'
import './locations.css'

const DEFAULT_CAP = 500_000_000_000
const empty = (): HostedLocation => ({ id: '', name: '', path: '', friendIds: [], rights: { upload: true, manage: false }, byteCap: DEFAULT_CAP })
/** The folder's own name, so "where is it?" already answers "what's it called?". */
const nameFromPath = (path: string): string => path.split('/').filter(Boolean).pop() || 'Shared folder'
const room = (m: { freeBytes?: number | null; totalBytes?: number | null }): string =>
  m.freeBytes != null && m.totalBytes ? ` · ${formatBytes(m.freeBytes)} free of ${formatBytes(m.totalBytes)}` : ''
/** What a friend may do, as plain-language choices instead of two checkboxes. */
const ACCESS = [
  { id: 'read', label: 'Can look and download', hint: 'They can open the folder and copy things out of it.', rights: { upload: false, manage: false } },
  { id: 'add', label: 'Can add files', hint: 'They can also put files and folders in. Nothing already there is overwritten.', rights: { upload: true, manage: false } },
  { id: 'manage', label: 'Can also rename & delete', hint: 'Full run of the folder. Anything they delete moves to the folder’s Trash, so it can be recovered.', rights: { upload: true, manage: true } },
] as const
const accessOf = (rights: { upload: boolean; manage: boolean }): string =>
  rights.manage ? 'manage' : rights.upload ? 'add' : 'read'

/**
 * Adding a folder used to be one form with a path field, a byte cap and two
 * permission checkboxes — fine if you already know what a mount point is. This
 * asks the three questions a person actually has, one at a time, and offers the
 * NAS shares and drives this device can already see as one-click answers.
 */
function AddLocationWizard({ onCancel, onSaved }: { onCancel: () => void; onSaved: (locations: HostedLocation[], addedId?: string) => void }) {
  const friends = useStore(s => s.friends)
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
  const stepNumber = step === 'where' ? 1 : step === 'name' ? 2 : 3
  return <form className="card location-editor location-wizard" onSubmit={e => {
    e.preventDefault()
    if (step === 'where') return
    if (step === 'name') { if (draft.name.trim()) setStep('who'); return }
    void save()
  }}>
    <div className="location-heading"><div>
      <h3>{step === 'where' ? 'Where is the folder?' : step === 'name' ? 'Name it' : 'Who can use it?'}</h3>
      <p className="location-muted">Step {stepNumber} of 3</p></div>
      <button type="button" className="icon-btn" disabled={busy} aria-label="Cancel adding a location" onClick={onCancel}><X size={17} /></button></div>
    {error && <p className="location-error" role="alert">{error}</p>}

    {step === 'where' && <>
      <p className="location-muted">Pick the drive or network share you want to share. Everything in it stays exactly where it is — DropBeam only opens a door to it for the friends you choose.</p>
      {mounts === null && <p className="location-muted">Looking for drives and network shares…</p>}
      <div className="location-wizard-choices">
        {(mounts || []).map(m => <button type="button" className="location-wizard-choice" key={m.path} onClick={() => choose(m.path, m.label)}>
          <span className="location-drive-icon">{m.kind === 'network' ? <Server size={21} /> : <HardDrive size={21} />}</span>
          <span className="location-grow">
            <strong>{m.label}</strong>
            <small>{m.kind === 'network' ? 'NAS / network drive' : 'External disk'}{room(m)}</small>
            <small className="location-path" title={m.path}>{m.path}</small>
          </span>
          <ChevronRight size={17} />
        </button>)}
        <button type="button" className="location-wizard-choice" onClick={() => { void browse() }}>
          <span className="location-drive-icon"><FolderOpen size={21} /></span>
          <span className="location-grow"><strong>Choose another folder…</strong>
            <small>{mounts && !mounts.length ? 'No drives or shares found — pick any folder on this device.' : 'Any folder on this device.'}</small></span>
          <ChevronRight size={17} />
        </button>
      </div>
      <div className="location-toolbar"><button type="button" className="btn btn-ghost" onClick={onCancel}>Cancel</button></div>
    </>}

    {step === 'name' && <>
      <label>What should your friends call it?
        <input autoFocus required maxLength={80} placeholder="Buddy NAS" value={draft.name}
          onChange={e => setDraft({ ...draft, name: e.target.value })} /></label>
      <p className="location-muted">This is the name your friends see. The folder itself isn’t renamed.</p>
      <p className="location-path" title={draft.path}>{draft.path}</p>
      <div className="location-toolbar">
        <button className="btn btn-primary" disabled={!draft.name.trim()}>Next<ChevronRight size={15} /></button>
        <button type="button" className="btn btn-ghost" onClick={() => setStep('where')}><ChevronLeft size={15} />Back</button>
      </div>
    </>}

    {step === 'who' && <>
      <fieldset><legend>People</legend>
        <p className="location-muted">Only the devices you tick here. Each of your own devices counts as one.</p>
        {!friends.length && <p className="location-muted">No friends added yet — you can share this folder now and pick people later.</p>}
        <div className="location-friend-list">{friends.map(f => <label className="location-check" key={f.id}>
          <input type="checkbox" checked={draft.friendIds.includes(f.id)} onChange={e => setDraft({ ...draft,
            friendIds: e.target.checked ? [...draft.friendIds, f.id] : draft.friendIds.filter(id => id !== f.id) })} />{f.name}</label>)}</div>
      </fieldset>
      <fieldset><legend>What they can do</legend>
        {ACCESS.map(option => <label className="location-check location-wizard-access" key={option.id}>
          <input type="radio" name="location-access" checked={access === option.id}
            onChange={() => setDraft({ ...draft, rights: { ...option.rights } })} />
          <span><strong>{option.label}</strong><small>{option.hint}</small></span>
        </label>)}
      </fieldset>
      <details className="location-wizard-advanced">
        <summary>Advanced</summary>
        <label>Largest single transfer (GB)
          <input type="number" min="0.001" step="0.001" required value={(draft.byteCap ?? DEFAULT_CAP) / 1_000_000_000}
            onChange={e => setDraft({ ...draft, byteCap: Math.max(1, Math.round(Number(e.target.value) * 1_000_000_000)) })} /></label>
        <p className="location-muted">A safety net: one upload or download can’t move more than this at a time. 500 GB suits most NAS folders.</p>
        <p className="location-muted">On save, DropBeam writes a tiny marker file in the folder so it can tell this exact drive from an empty mount point later, and checks whether the filesystem supports safe (never-overwrite) publishing.</p>
      </details>
      <div className="location-toolbar">
        <button className="btn btn-primary" disabled={busy || !draft.name.trim() || !draft.path.trim()}>{busy ? 'Sharing…' : 'Share this folder'}</button>
        <button type="button" className="btn btn-ghost" disabled={busy} onClick={() => setStep('name')}><ChevronLeft size={15} />Back</button>
      </div>
    </>}
  </form>
}

export function LocationSettings() {
  const friends = useStore(s => s.friends)
  const [locations, setLocations] = useState<HostedLocation[]>([])
  const [draft, setDraft] = useState<HostedLocation | null>(null)
  const [adding, setAdding] = useState(false)
  const [added, setAdded] = useState('')
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState('')
  const [activity, setActivity] = useState<LocationActivity[]>([])
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
  const save = async () => {
    if (!draft || busy) return
    setBusy(true); setError('')
    try { setLocations(await locationsApi.save(draft)); setDraft(null) }
    catch (e) { setError(String(e)) }
    finally { setBusy(false) }
  }
  return <section className="location-settings" aria-label="Locations settings">
    <div className="location-heading"><div><h2><HardDrive size={18} /> Locations</h2>
      <p>Share a folder or mounted NAS with the friends you choose.</p></div>
      <button className="btn btn-ghost" disabled={!loaded || busy || adding} onClick={() => { setError(''); setAdded(''); setDraft(null); setAdding(true) }}><Plus size={15} /> Add a location</button></div>
    {error && <p className="location-error" role="alert">{error}</p>}
    {!loaded && !error && <p className="location-muted">Loading locations…</p>}
    {loaded && locations.length === 0 && !draft && !adding && <div className="card location-empty-small">No folders shared yet. A new location stays private until you pick friends for it.</div>}
    {locations.map(l => <div className={`card location-host-row${l.id === added ? ' location-host-row-new' : ''}`} key={l.id}>
      <HardDrive size={23} /><div className="location-grow"><strong>{l.name}</strong><div className="location-path">{l.path}</div>
        {l.id === added && <small className="location-reach ok"><Check size={13} /> Shared. Your friends will find it under Locations.</small>}
        <small>{l.friendIds.length
          ? `${l.friendIds.length} friend${l.friendIds.length === 1 ? '' : 's'} · ${ACCESS.find(a => a.id === accessOf(l.rights))?.label ?? 'Can look and download'}`
          : 'Private · Nobody has access yet'}</small>
        <details className="location-wizard-advanced"><summary>Advanced</summary>
          <small>Largest single transfer: {(l.byteCap ?? DEFAULT_CAP) / 1_000_000_000} GB · Safe publish: {l.safePublish || 'save to probe'}</small></details></div>
      <button className="btn btn-ghost" disabled={busy} onClick={() => { setError(''); setAdding(false); setDraft({ ...l, friendIds: [...l.friendIds], rights: { ...l.rights } }) }}>Edit</button>
      <button className="btn btn-ghost" disabled={busy} onClick={async () => {
        setBusy(true); setError('')
        try { setLocations(await locationsApi.remove(l.id)); if (draft?.id === l.id) setDraft(null) }
        catch(e) { setError(String(e)) } finally { setBusy(false) }
      }}>Stop sharing</button>
    </div>)}
    {adding && <AddLocationWizard onCancel={() => setAdding(false)}
      onSaved={(list, id) => { setLocations(list); setAdding(false); setAdded(id || '') }} />}
    {draft && <form className="card location-editor" onSubmit={e => { e.preventDefault(); void save() }}>
      <div className="location-heading"><h3>Edit location</h3><button type="button" className="icon-btn" disabled={busy} aria-label="Close location editor" onClick={() => setDraft(null)}><X size={17} /></button></div>
      <label>Display name<input autoFocus required maxLength={80} placeholder="Family NAS" value={draft.name} onChange={e => setDraft({ ...draft, name: e.target.value })} /></label>
      <label>Local folder path<div className="location-toolbar"><input required placeholder="/run/user/1000/gvfs/sftp:host=buddy-files/shares" value={draft.path} onChange={e => setDraft({ ...draft, path: e.target.value })} />
        <button type="button" className="btn btn-ghost" onClick={async () => {
          try { const path = await api.pickDirectory(); if (path) setDraft(d => d ? { ...d, path, name: d.name || nameFromPath(path) } : d) } catch(e) { setError(String(e)) }
        }}><FolderOpen size={16} /> Choose</button></div></label>
      <label>Byte cap per upload / download (GB)<input type="number" min="0.001" step="0.001" required value={(draft.byteCap ?? DEFAULT_CAP) / 1_000_000_000} onChange={e => setDraft({ ...draft, byteCap: Math.round(Number(e.target.value) * 1_000_000_000) })} /></label>
      <fieldset><legend>Who can access this location?</legend><p className="location-muted">Only these friend devices. Select each of your devices separately.</p>
        {!friends.length && <p>Add a friend first, or save this location privately.</p>}
        <div className="location-friend-list">{friends.map(f => <label className="location-check" key={f.id}><input type="checkbox" checked={draft.friendIds.includes(f.id)} onChange={e => setDraft({ ...draft, friendIds: e.target.checked ? [...draft.friendIds, f.id] : draft.friendIds.filter(id => id !== f.id) })} />{f.name}</label>)}</div>
      </fieldset>
      <fieldset><legend>Permissions</legend>
        <p className="location-muted">Selected friends can always browse and download.</p>
        <label className="location-check"><input type="checkbox" checked={draft.rights.upload} onChange={e => setDraft({ ...draft, rights: { ...draft.rights, upload: e.target.checked } })} />Upload files and folders</label>
        <label className="location-check"><input type="checkbox" checked={draft.rights.manage} onChange={e => setDraft({ ...draft, rights: { ...draft.rights, manage: e.target.checked } })} />Manage: new folder, rename, move to trash</label>
        <p className="location-muted">A file that is already there is never overwritten: a changed copy lands beside it as “… (2)”, unless the sender is keeping a folder in sync, in which case the older version moves to this location’s Trash. Move to trash keeps recoverable items in this location’s .dropbeam-trash folder. Stop sharing only removes access.</p>
      </fieldset>
      <div className="location-toolbar"><button className="btn btn-primary" disabled={busy || !draft.name.trim() || !draft.path.trim()}>{busy ? 'Saving…' : 'Save location'}</button><button type="button" className="btn btn-ghost" disabled={busy} onClick={() => setDraft(null)}>Cancel</button></div>
    </form>}
    <div className="card location-editor"><h3>Recent activity</h3>{!activity.length && <p className="location-muted">Folder changes during this app session appear here (last 50).</p>}
      {activity.map((a, i) => <div key={`${a.at}:${i}`}><small>{new Date(a.at).toLocaleTimeString()} · {friends.find(f => f.id === a.friendId)?.name || a.friendId} · {locations.find(l => l.id === a.locationId)?.name || a.locationId}</small><div>{a.operation.replace('locations.', '')}: {a.item}{a.to ? ` → ${a.to}` : ''}</div></div>)}
    </div>
  </section>
}
