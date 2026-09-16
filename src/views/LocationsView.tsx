import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { AlertTriangle, ArrowLeft, ArrowRight, Check, Download, HardDrive, Plus, RefreshCw, Settings2, Share2, Upload } from 'lucide-react'
import { locationsApi, onLocationsChanged, type HostedLocation, type HostedLocationStatus, type SharedLocation } from '../lib/api'
import { formatBytes, formatRelativeTime } from '../lib/format'
import { incomingByLocation, trackLocationTransfers } from '../lib/hostedLocations'
import { claimPresenceChecks, friendPresence, presenceLabel } from '../lib/presence'
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
/**
 * The HOST's half of Locations. A device that hosts a NAS folder never browses
 * that folder through DropBeam, so nothing here ever said "this device is the
 * gateway your friends upload through". One card per hosted folder: where it
 * lives, who may reach it and with what rights, whether the mount is actually
 * there right now, what last happened in it, and a live line while a friend is
 * pushing files in.
 */
function SharedFromThisDevice() {
  const friends = useStore(s => s.friends)
  const setView = useStore(s => s.setView)
  const transfers = useStore(s => s.transfers)
  const [hosted, setHosted] = useState<HostedLocation[]>([])
  const [statuses, setStatuses] = useState<Record<string, HostedLocationStatus>>({})
  const [error, setError] = useState('')
  const [loaded, setLoaded] = useState(false)
  const [known, setKnown] = useState<Record<string, string>>({})
  const mounted = useRef(false)
  useEffect(() => setKnown(prev => trackLocationTransfers(prev, transfers)), [transfers])

  const reload = useCallback(async () => {
    try {
      const list = await locationsApi.listHosted()
      if (!mounted.current) return
      setHosted(list); setLoaded(true); setError('')
      return list
    } catch (e) { if (mounted.current) { setError(String(e)); setLoaded(true) } }
  }, [])
  // Status is cheap (one open + marker read + statvfs), so it can follow the
  // view: once on open, then every 30 s while it stays open.
  const probe = useCallback(async (list: HostedLocation[]) => {
    await Promise.all(list.map(async l => {
      try {
        const status = await locationsApi.hostedStatus(l.id)
        if (mounted.current) setStatuses(prev => ({ ...prev, [l.id]: status }))
      } catch (e) {
        if (mounted.current) setStatuses(prev => ({ ...prev, [l.id]: { id: l.id, reachable: false, freeBytes: 0, markerOk: false, error: String(e), lastActivity: prev[l.id]?.lastActivity ?? null } }))
      }
    }))
  }, [])
  useEffect(() => {
    mounted.current = true
    const tick = async () => { const list = await reload(); if (list) await probe(list) }
    void tick()
    const interval = setInterval(() => { void tick() }, 30_000)
    let un: (() => void) | undefined
    void onLocationsChanged(() => { void tick() }).then(fn => { if (mounted.current) un = fn; else fn() })
    return () => { mounted.current = false; clearInterval(interval); un?.() }
  }, [reload, probe])

  // Live inbound uploads, grouped by the folder they're landing in.
  const incoming = useMemo(() => incomingByLocation(transfers, known), [transfers, known])

  const add = <button className="btn btn-primary" onClick={() => setView('settings')}><Plus size={15} /> Add a location</button>
  return <section className="location-hosted" aria-label="Folders shared from this device">
    <div className="location-heading"><div><h2><Share2 size={17} /> Shared from this device</h2>
      <p>Folders this device hands out to friends. It stays the gateway — friends reach them only while it’s awake.</p></div>
      {!!hosted.length && <button className="btn btn-ghost" onClick={() => setView('settings')}><Settings2 size={15} /> Manage</button>}</div>
    {error && <p className="location-error" role="alert">{error}</p>}
    {!loaded && !error && <p className="location-muted">Checking what this device shares…</p>}
    {loaded && !hosted.length && !error && <div className="card location-empty-small">
      <h3>This device isn’t sharing any folders yet</h3>
      <p className="location-muted">Share a NAS mount or a local folder and the friends you pick can browse, download and upload to it.</p>
      {add}</div>}
    <div className="location-grid">{hosted.map(l => {
      const status = statuses[l.id]
      const live = incoming[l.id]
      const members = l.friendIds.map(id => friends.find(f => f.id === id)?.name || 'Unknown device')
      const rights = `Browse & download${l.rights.upload ? ' · Upload' : ''}${l.rights.manage ? ' · Manage' : ''}`
      const last = status?.lastActivity
      return <div className="card location-tile location-gateway" key={l.id}>
        <div className="location-tile-top"><span className="location-drive-icon"><HardDrive size={24} /></span>
          <span className="location-badge"><Share2 size={11} /> Gateway</span></div>
        <h2>{l.name}</h2>
        <p className="location-path" title={l.path}>{l.path}</p>
        <div className={`location-reach ${status ? (status.reachable && status.markerOk ? 'ok' : 'warn') : ''}`}>
          {!status ? <>Checking this folder…</>
            : status.reachable && status.markerOk
              ? <><Check size={13} /> Reachable{status.freeBytes > 0 ? ` · ${formatBytes(status.freeBytes)} free` : ''}</>
              : <><AlertTriangle size={13} /> {status.reachable ? 'Mount changed since you shared it' : 'Not reachable right now'}{status.error ? ` — ${status.error}` : ''}</>}
        </div>
        {live && <div className="location-live" role="status">
          <Upload size={13} /> Receiving from {live.friends.join(', ')} · {live.files} file{live.files === 1 ? '' : 's'}{live.bytes > 0 ? ` · ${formatBytes(live.bytes)}` : ''}
        </div>}
        <div className="location-members">{members.length
          ? members.map((name, i) => <span className="location-chip" key={`${name}:${i}`}>{name}</span>)
          : <span className="location-muted">Private · nobody has access yet</span>}</div>
        <div className="location-tile-bottom"><span>{members.length ? rights : 'Pick friends in Settings to share it'}</span></div>
        <small className="location-muted location-last">{last
          ? <>{last.direction === 'download' ? <Download size={11} /> : <Upload size={11} />} {friends.find(f => f.id === last.friendId)?.name || 'A friend'} {last.direction === 'download' ? 'downloaded' : 'uploaded'} {formatBytes(last.bytes)} · {formatRelativeTime(last.at)}</>
          : 'No friend activity recorded yet'}</small>
      </div>
    })}</div>
  </section>
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
  // #34: this view only asks friends it believes are ONLINE for their locations,
  // so a stale "offline" hid their folders until a restart. Re-check on open.
  useEffect(() => {
    const s = useStore.getState()
    for (const id of claimPresenceChecks(s.friends, s.friendSeen, s.folderStatuses)) void s.pingFriend(id)
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
    <div className="location-heading"><div><h1>Locations</h1><p>Your friends’ folders, within reach.</p></div><div className="location-toolbar">
      <button className="btn btn-ghost" disabled={busy} onClick={() => { void refresh() }}><RefreshCw size={15} className={busy ? 'location-spin' : ''} />Refresh</button>
      <button className="btn btn-ghost" onClick={() => setView('settings')}><Settings2 size={15} />Share a folder</button></div></div>
    {active ? <><button className="btn btn-ghost location-back" onClick={() => setActive(null)}><ArrowLeft size={16} />All locations</button>
      {friend && location ? <><div className="location-host-label"><HardDrive size={16} /><strong>{friend.name}</strong><span> / {location.name}</span><span className="location-grow" /><button className="btn btn-ghost" onClick={() => setView('send')}>Send & Receive<ArrowRight size={14} /></button></div>
        <FileBrowser key={`${friend.id}:${location.id}`} friendId={friend.id} location={location} online={presenceLabel(friendPresence(friend.name, seen, statuses))} /></> : <div className="card location-empty-small">This location is no longer shared with this device.</div>}</> : <>
      {!count && <div className="card location-empty"><HardDrive size={46} strokeWidth={1.1} /><h2>{busy ? 'Looking for shared locations…' : 'A place for everything'}</h2><p>When a friend shares a folder with this device, it appears here.<br/>To share your NAS or a local folder, add it in Settings → Locations.</p><button className="btn btn-primary" onClick={() => setView('settings')}>Set up a location<ArrowRight size={15} /></button></div>}
      <div className="location-grid">{friends.flatMap(f => (shared[f.id] || []).map(l => {
        const presence = friendPresence(f.name, seen, statuses)
        return <button className="card location-tile" key={`${f.id}:${l.id}`} onClick={() => setActive({ friend: f.id, location: l.id })}>
          <div className="location-tile-top"><span className="location-drive-icon"><HardDrive size={24} /></span><span className={`location-presence ${presence.status}`}><i />{presenceLabel(presence)}</span></div>
          <h2>{l.name}</h2><p>{f.name}</p><div className="location-tile-bottom"><span>Browse & download{l.rights.upload ? ' · Upload' : ''}{l.rights.manage ? ' · Manage' : ''}</span><ArrowRight size={17} /></div>
          {errors[f.id] && <small className="location-muted">Last known location · Open to retry</small>}
        </button>
      }))}</div>
      {!!Object.keys(errors).length && <details className="location-connection-details"><summary>{Object.keys(errors).length} device(s) unavailable or without Locations support</summary>{friends.filter(f => errors[f.id]).map(f => <p key={f.id}><strong>{f.name}:</strong> {errors[f.id]}</p>)}</details>}
      <hr className="location-divider" />
      <SharedFromThisDevice />
    </>}
  </div>
}
