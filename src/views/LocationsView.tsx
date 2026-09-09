import { MOBILE_UI } from '../lib/platform'
import { useCallback, useEffect, useRef, useState } from 'react'
import { ArrowLeft, ArrowRight, HardDrive, RefreshCw, Settings2 } from 'lucide-react'
import { locationsApi, onLocationsChanged, type SharedLocation } from '../lib/api'
import { friendPresence, presenceLabel } from '../lib/presence'
import { useStore } from '../store'
import { FileBrowser } from '../components/FileBrowser'
import '../components/locations.css'

type SharedByFriend = Record<string, SharedLocation[]>
function cached(): SharedByFriend {
  try {
    const value = JSON.parse(localStorage.getItem('dropbeam.locations') || '{}')
    if (!value || typeof value !== 'object' || Array.isArray(value)) return {}
    return Object.fromEntries(Object.entries(value).filter(([, v]) => Array.isArray(v) && v.every(l => l && typeof l.id === 'string' && typeof l.name === 'string' && l.rights && typeof l.rights.upload === 'boolean' && typeof l.rights.manage === 'boolean'))) as SharedByFriend
  } catch { return {} }
}
export function LocationsView() {
  const friends = useStore(s => s.friends)
  const seen = useStore(s => s.friendSeen)
  const statuses = useStore(s => s.folderStatuses)
  const setView = useStore(s => s.setView)
  const [shared, setShared] = useState<SharedByFriend>(cached)
  const [errors, setErrors] = useState<Record<string, string>>({})
  const [busy, setBusy] = useState(false)
  const [active, setActive] = useState<{ friend: string; location: string } | null>(null)
  const mounted = useRef(false)
  const inFlight = useRef(false)
  const queued = useRef(false)
  const friendKey = friends.map(f => `${f.id}:${f.endpointId}`).join('|')
  const presenceKey = friends.map(f => `${f.id}:${friendPresence(f.name, seen, statuses).status}`).join('|')
  const refresh = useCallback(async function reload() {
    if (inFlight.current) { queued.current = true; return }
    inFlight.current = true; setBusy(true)
    const state = useStore.getState()
    await Promise.all(state.friends.filter(f => f.endpointId && friendPresence(f.name, state.friendSeen, state.folderStatuses).status === 'online').map(async f => {
      try {
        const locations = await locationsApi.list(f.id)
        if (!mounted.current) return
        setShared(prev => ({ ...prev, [f.id]: locations }))
        setErrors(prev => { const next = { ...prev }; delete next[f.id]; return next })
      } catch(e) {
        if (mounted.current) setErrors(prev => ({ ...prev, [f.id]: String(e) }))
      }
    }))
    if (mounted.current) setBusy(false)
    inFlight.current = false
    if (mounted.current && queued.current) { queued.current = false; void reload() }
  }, [])
  useEffect(() => {
    mounted.current = true; void refresh()
    const interval = setInterval(() => { void refresh() }, 30_000)
    let un: (() => void) | undefined
    void onLocationsChanged(() => { void refresh() }).then(fn => { if (mounted.current) un = fn; else fn() })
    return () => { mounted.current = false; clearInterval(interval); un?.() }
  }, [refresh, friendKey, presenceKey])
  useEffect(() => {
    const ids = new Set(friends.map(f => f.id))
    try { localStorage.setItem('dropbeam.locations', JSON.stringify(Object.fromEntries(Object.entries(shared).filter(([id]) => ids.has(id))))) } catch { /* cache is optional */ }
  }, [friends, shared])
  useEffect(() => {
    const drop = () => { if (!active) useStore.getState().toast('info', 'Open a location to upload files there.') }
    window.addEventListener('dropbeam:location-drop', drop)
    return () => window.removeEventListener('dropbeam:location-drop', drop)
  }, [active])
  const friend = friends.find(f => f.id === active?.friend)
  const location = active && shared[active.friend]?.find(l => l.id === active.location)
  const count = friends.reduce((sum,f) => sum + (shared[f.id]?.length || 0), 0)
  return <div className="locations-view">
    {MOBILE_UI && <button className="btn btn-ghost location-back" onClick={() => setView('friends')}><ArrowLeft size={16} />Friends</button>}
    <div className="location-heading"><div><h1>Locations</h1><p>Your friends’ folders, within reach.</p></div><div className="location-toolbar">
      <button className="btn btn-ghost" disabled={busy} onClick={() => { void refresh() }}><RefreshCw size={15} className={busy ? 'location-spin' : ''} />Refresh</button>
      {!MOBILE_UI && <button className="btn btn-ghost" onClick={() => setView('settings')}><Settings2 size={15} />Share a folder</button>}</div></div>
    {active ? <><button className="btn btn-ghost location-back" onClick={() => setActive(null)}><ArrowLeft size={16} />All locations</button>
      {friend && location ? <><div className="location-host-label"><HardDrive size={16} /><strong>{friend.name}</strong><span> / {location.name}</span><span className="location-grow" /><button className="btn btn-ghost" onClick={() => setView('send')}>Send & Receive<ArrowRight size={14} /></button></div>
        <FileBrowser key={`${friend.id}:${location.id}`} friendId={friend.id} location={location} online={presenceLabel(friendPresence(friend.name, seen, statuses))} /></> : <div className="card location-empty-small">This location is no longer shared with this device.</div>}</> : <>
      {!count && <div className="card location-empty"><HardDrive size={46} strokeWidth={1.1} /><h2>{busy ? 'Looking for shared locations…' : 'A place for everything'}</h2><p>When a friend shares a folder with this device, it appears here.<br/>{MOBILE_UI ? 'Share a folder from DropBeam on your computer to browse it here.' : 'To share your NAS or a local folder, add it in Settings → Locations.'}</p>{!MOBILE_UI && <button className="btn btn-primary" onClick={() => setView('settings')}>Set up a location<ArrowRight size={15} /></button>}</div>}
      <div className="location-grid">{friends.flatMap(f => (shared[f.id] || []).map(l => {
        const presence = friendPresence(f.name, seen, statuses)
        return <button className="card location-tile" key={`${f.id}:${l.id}`} onClick={() => setActive({ friend: f.id, location: l.id })}>
          <div className="location-tile-top"><span className="location-drive-icon"><HardDrive size={24} /></span><span className={`location-presence ${presence.status}`}><i />{presenceLabel(presence)}</span></div>
          <h2>{l.name}</h2><p>{f.name}</p><div className="location-tile-bottom"><span>Browse & download{l.rights.upload ? ' · Upload' : ''}{l.rights.manage ? ' · Manage' : ''}</span><ArrowRight size={17} /></div>
          {errors[f.id] && <small className="location-muted">Last known location · Open to retry</small>}
        </button>
      }))}</div>
      {!!Object.keys(errors).length && <details className="location-connection-details"><summary>{Object.keys(errors).length} device(s) unavailable or without Locations support</summary>{friends.filter(f => errors[f.id]).map(f => <p key={f.id}><strong>{f.name}:</strong> {errors[f.id]}</p>)}</details>}
    </>}
  </div>
}
