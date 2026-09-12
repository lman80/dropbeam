import { useEffect, useState } from 'react'
import { FolderOpen, HardDrive, Plus, X } from 'lucide-react'
import { api, locationsApi, onLocationActivity, type LocationActivity, type HostedLocation } from '../lib/api'
import { useStore } from '../store'
import './locations.css'

const empty = (): HostedLocation => ({ id: '', name: '', path: '', friendIds: [], rights: { upload: true, manage: false }, byteCap: 500_000_000_000 })
export function LocationSettings() {
  const friends = useStore(s => s.friends)
  const [locations, setLocations] = useState<HostedLocation[]>([])
  const [draft, setDraft] = useState<HostedLocation | null>(null)
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
      <button className="btn btn-ghost" disabled={!loaded || busy} onClick={() => { setError(''); setDraft(empty()) }}><Plus size={15} /> Add location</button></div>
    {error && <p className="location-error" role="alert">{error}</p>}
    {!loaded && !error && <p className="location-muted">Loading locations…</p>}
    {loaded && locations.length === 0 && !draft && <div className="card location-empty-small">No folders shared. New locations are private until you select friends.</div>}
    {locations.map(l => <div className="card location-host-row" key={l.id}>
      <HardDrive size={23} /><div className="location-grow"><strong>{l.name}</strong><div className="location-path">{l.path}</div>
        <small>{l.friendIds.length ? `${l.friendIds.length} friend${l.friendIds.length === 1 ? '' : 's'} · Browse & download${l.rights.upload ? ' · Upload' : ''}${l.rights.manage ? ' · Manage' : ''}` : 'Private · Nobody has access'}</small><div><small>Safe publish: {l.safePublish || 'save to probe'} · Cap: {(l.byteCap ?? 500_000_000_000) / 1_000_000_000} GB per transfer</small></div></div>
      <button className="btn btn-ghost" disabled={busy} onClick={() => { setError(''); setDraft({ ...l, friendIds: [...l.friendIds], rights: { ...l.rights } }) }}>Edit</button>
      <button className="btn btn-ghost" disabled={busy} onClick={async () => {
        setBusy(true); setError('')
        try { setLocations(await locationsApi.remove(l.id)); if (draft?.id === l.id) setDraft(null) }
        catch(e) { setError(String(e)) } finally { setBusy(false) }
      }}>Stop sharing</button>
    </div>)}
    {draft && <form className="card location-editor" onSubmit={e => { e.preventDefault(); void save() }}>
      <div className="location-heading"><h3>{draft.id ? 'Edit location' : 'New location'}</h3><button type="button" className="icon-btn" disabled={busy} aria-label="Close location editor" onClick={() => setDraft(null)}><X size={17} /></button></div>
      <label>Display name<input autoFocus required maxLength={80} placeholder="Family NAS" value={draft.name} onChange={e => setDraft({ ...draft, name: e.target.value })} /></label>
      <label>Local folder path<div className="location-toolbar"><input required placeholder="/run/user/1000/gvfs/sftp:host=buddy-files/shares" value={draft.path} onChange={e => setDraft({ ...draft, path: e.target.value })} />
        <button type="button" className="btn btn-ghost" onClick={async () => {
          try { const path = await api.pickDirectory(); if (path) setDraft(d => d ? { ...d, path, name: d.name || path.split('/').filter(Boolean).pop() || 'Location' } : d) } catch(e) { setError(String(e)) }
        }}><FolderOpen size={16} /> Choose</button></div></label>
      <label>Byte cap per upload / download (GB)<input type="number" min="0.001" step="0.001" required value={(draft.byteCap ?? 500_000_000_000) / 1_000_000_000} onChange={e => setDraft({ ...draft, byteCap: Math.round(Number(e.target.value) * 1_000_000_000) })} /></label>
      <fieldset><legend>Who can access this location?</legend><p className="location-muted">Only these friend devices. Select each of your devices separately.</p>
        {!friends.length && <p>Add a friend first, or save this location privately.</p>}
        <div className="location-friend-list">{friends.map(f => <label className="location-check" key={f.id}><input type="checkbox" checked={draft.friendIds.includes(f.id)} onChange={e => setDraft({ ...draft, friendIds: e.target.checked ? [...draft.friendIds, f.id] : draft.friendIds.filter(id => id !== f.id) })} />{f.name}</label>)}</div>
      </fieldset>
      <fieldset><legend>Permissions</legend>
        <p className="location-muted">Selected friends can always browse and download.</p>
        <label className="location-check"><input type="checkbox" checked={draft.rights.upload} onChange={e => setDraft({ ...draft, rights: { ...draft.rights, upload: e.target.checked } })} />Upload files and folders</label>
        <label className="location-check"><input type="checkbox" checked={draft.rights.manage} onChange={e => setDraft({ ...draft, rights: { ...draft.rights, manage: e.target.checked } })} />Manage: new folder, rename, move to trash</label>
        <p className="location-muted">Existing files are never overwritten. Move to trash keeps recoverable items in this location’s .dropbeam-trash folder. Stop sharing only removes access.</p>
      </fieldset>
      <div className="location-toolbar"><button className="btn btn-primary" disabled={busy || !draft.name.trim() || !draft.path.trim()}>{busy ? 'Saving…' : 'Save location'}</button><button type="button" className="btn btn-ghost" disabled={busy} onClick={() => setDraft(null)}>Cancel</button></div>
    </form>}
    <div className="card location-editor"><h3>Recent activity</h3>{!activity.length && <p className="location-muted">Folder changes during this app session appear here (last 50).</p>}
      {activity.map((a, i) => <div key={`${a.at}:${i}`}><small>{new Date(a.at).toLocaleTimeString()} · {friends.find(f => f.id === a.friendId)?.name || a.friendId} · {locations.find(l => l.id === a.locationId)?.name || a.locationId}</small><div>{a.operation.replace('locations.', '')}: {a.item}{a.to ? ` → ${a.to}` : ''}</div></div>)}
    </div>
  </section>
}
