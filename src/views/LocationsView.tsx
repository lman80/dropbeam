import { loadLocations } from '../lib/locationsLoad'
import { MobileHeader } from '../components/MobileHeader'
import { IS_WINDOWS, MOBILE_UI } from '../lib/platform'
import { useCallback, useEffect, useMemo, useRef, useState } from 'react'
import { ChevronLeft, ChevronRight, HardDrive, Info, RefreshCw } from 'lucide-react'
import { locationsApi, onLocationsChanged, type HostedLocation, type HostedLocationStatus, type SharedLocation } from '../lib/api'
import { formatBytes, formatRelativeTime } from '../lib/format'
import { incomingByLocation, trackLocationTransfers } from '../lib/hostedLocations'
import { claimPresenceChecks, friendPresence, presenceLabel } from '../lib/presence'
import { useStore } from '../store'
import { FileBrowser } from '../components/FileBrowser'
import { SyncedFolders, openSyncFolderSheet } from '../components/SyncedFolders'
import { peopleLabel, accessLabel } from '../components/LocationSettings'
import { Dot, EmptyState, IconButton, InfoButton, SectionHeader, Spinner } from '../components/ui'

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

  if (loaded && !hosted.length && !error) return null
  return <section className="location-section" aria-label="Shared from this device">
    <SectionHeader action={hosted.length > 0 && <button className="btn btn-plain btn-sm" onClick={() => setView('settings')}>Manage in Settings</button>}>
      Shared from this device</SectionHeader>
    {error && <p className="form-error" role="alert">{error}</p>}
    {hosted.length > 0 && <div className="group">{hosted.map(l => {
      const status = statuses[l.id]
      const live = incoming[l.id]
      const members = l.friendIds.map(id => friends.find(f => f.id === id)?.name || 'a removed device')
      const last = status?.lastActivity
      const ok = !!status && status.reachable && status.markerOk
      const reach = !status ? null
        : ok ? <span className="location-reach" title={status.freeBytes > 0 ? `${formatBytes(status.freeBytes)} free` : undefined}><Dot tone="ok" /><span className="truncate-1">Available{status.freeBytes > 0 ? ` · ${formatBytes(status.freeBytes)} free` : ''}</span></span>
          : <span className="location-reach warn" title={status.error || (status.reachable ? 'The drive at this path isn’t the one you shared.' : undefined)}>
            <Dot tone="warn" /><span className="truncate-1">{status.reachable ? 'Drive changed' : 'Not reachable'}</span></span>
      return <div className="row location-row" key={l.id}>
        <span className="location-glyph" aria-hidden><HardDrive /></span>
        <div className="row-main">
          <div className="row-title truncate-1" title={l.path}>{l.name}</div>
          <div className="row-sub truncate-1" title={members.join(', ')}>{members.length ? `${peopleLabel(members)} · ${accessLabel(l.rights)}` : 'Only you'}</div>
          {live
            ? <div className="row-sub location-activity live truncate-1" role="status"><Spinner size={10} /> Receiving {live.files === 1 ? '1 file' : `${live.files} files`} from {peopleLabel(live.friends)}{live.bytes > 0 ? ` · ${formatBytes(live.bytes)}` : ''}</div>
            : last && <div className="row-sub location-activity truncate-1">
              {friends.find(f => f.id === last.friendId)?.name || 'A friend'} {last.direction === 'download' ? 'downloaded' : 'uploaded'} {formatBytes(last.bytes)} · {formatRelativeTime(last.at)}</div>}
        </div>
        <div className="row-trailing">{reach}</div>
      </div>
    })}</div>}
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
    await loadLocations({
      friends: state.friends.filter(f => friendPresence(f.name, state.friendSeen, state.folderStatuses).status === 'online'),
      online: () => true,
      list: locationsApi.list,
      errorText: (_friend, error) => String(error),
      onResult: result => {
        if (!mounted.current) return
        if (result.error) setErrors(prev => ({ ...prev, [result.friendId]: result.error! }))
        else {
          setShared(prev => ({ ...prev, [result.friendId]: result.locations }))
          setErrors(prev => { const next = { ...prev }; delete next[result.friendId]; return next })
        }
      },
    })
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
    // Cache the folders, never how much room they had: a remembered "1.2 TB
    // free" from last week would read as live. The next reply brings it back.
    const keep = Object.entries(shared).filter(([id]) => ids.has(id))
      .map(([id, list]) => [id, list.map(({ id: lid, name, rights }) => ({ id: lid, name, rights }))] as const)
    try { localStorage.setItem('dropbeam.locations', JSON.stringify(Object.fromEntries(keep))) } catch { /* cache is optional */ }
  }, [friends, shared])
  useEffect(() => {
    const drop = () => { if (!active) useStore.getState().toast('info', 'Open a location to upload files there.') }
    window.addEventListener('dropbeam:location-drop', drop)
    return () => window.removeEventListener('dropbeam:location-drop', drop)
  }, [active])
  const friend = friends.find(f => f.id === active?.friend)
  const location = active && shared[active.friend]?.find(l => l.id === active.location)
  const count = friends.reduce((sum,f) => sum + (shared[f.id]?.length || 0), 0)
  if (MOBILE_UI) return <div className="mobile-page mobile-locations">
    {active && friend && location ? <FileBrowser key={`${friend.id}:${location.id}`} friendId={friend.id} location={location} online={presenceLabel(friendPresence(friend.name, seen, statuses))} onBack={() => setActive(null)} /> : <>
      <MobileHeader title="Locations" actions={<button className="ios-icon" aria-label="Refresh locations" disabled={busy} onClick={() => void refresh()}><RefreshCw size={20} /></button>} />
      <div className="ios-list">{friends.flatMap(f => (shared[f.id] || []).map(l => <button className="ios-row" key={`${f.id}:${l.id}`} onClick={() => setActive({ friend: f.id, location: l.id })}><span className="mobile-tinted-icon"><HardDrive size={24} /></span><span className="mobile-grow"><span className="ios-headline mobile-ellipsis">{l.name}</span><span className="ios-footnote mobile-presence"><i className={friendPresence(f.name, seen, statuses).status === 'online' ? 'online' : ''} />{f.name} · {presenceLabel(friendPresence(f.name, seen, statuses))}</span></span><ChevronRight size={18} /></button>))}</div>
      {!count && <div className="mobile-empty"><HardDrive /><h2 className="ios-title2">A place for everything</h2><p className="ios-footnote">Folders shared by friends appear here.</p><button className="ios-button ios-primary" onClick={() => setView('friends')}>Find a friend</button></div>}
      {!!Object.keys(errors).length && <p className="ios-footnote mobile-inset">Some devices are unavailable. Open a location to retry.</p>}
    </>}
  </div>
  const presenceWords = (f: { name: string }) => {
    const p = friendPresence(f.name, seen, statuses)
    return { online: p.status === 'online', words: p.status === 'online' ? 'Online' : presenceLabel(p) }
  }
  if (active) return <div className="locations-view page">
    {friend && location
      ? <FileBrowser key={`${friend.id}:${location.id}`} friendId={friend.id} location={location} host={friend.name}
        online={presenceWords(friend).words} onBack={() => setActive(null)} />
      : <>
        <div className="page-header titlebar-drag"><div className="location-crumbs">
          <IconButton label="All locations" onClick={() => setActive(null)}><ChevronLeft /></IconButton>
          <h1 className="page-title">Locations</h1></div></div>
        <EmptyState icon={<HardDrive />} title="This location isn’t shared with you anymore" />
      </>}
  </div>
  const unavailable = friends.filter(f => errors[f.id])
  return <div className="locations-view page">
    <div className="page-header titlebar-drag"><h1 className="page-title">Locations</h1><div className="page-actions">
      {!IS_WINDOWS && <button className="btn btn-secondary" onClick={() => setView('settings')}>Share a folder…</button>}
      {count > 0 && <button className="btn btn-primary" onClick={openSyncFolderSheet}>Sync a folder…</button>}
    </div></div>

    {count > 0
      ? <section className="location-section" aria-label="Shared with you">
        <SectionHeader>Shared with you</SectionHeader>
        <div className="group">{friends.flatMap(f => (shared[f.id] || []).map(l => {
          const p = presenceWords(f)
          const room = l.reachable === false
            ? <span className="location-reach warn"><Dot tone="warn" />Not reachable</span>
            : l.reachable && l.freeBytes != null && l.totalBytes
              ? <span className="location-reach" title={`${formatBytes(l.freeBytes)} free of ${formatBytes(l.totalBytes)}`}>{formatBytes(l.freeBytes)} free</span>
              : null
          return <button type="button" className="row location-row location-tile" key={`${f.id}:${l.id}`} onClick={() => setActive({ friend: f.id, location: l.id })}>
            <span className="location-glyph" aria-hidden><HardDrive /></span>
            <span className="row-main">
              <span className="row-title truncate-1" title={l.name}>{l.name}</span>
              <span className="row-sub location-presence truncate-1"><Dot tone={p.online ? 'online' : 'off'} />{f.name} · {p.words}</span>
            </span>
            <span className="row-trailing">{room}<ChevronRight className="location-chevron" aria-hidden /></span>
          </button>
        }))}</div>
      </section>
      : <EmptyState icon={<HardDrive />} title={busy ? 'Looking for locations…' : 'No locations yet'}
        hint="Folders friends share with you appear here." style={{ padding: '40px 24px 32px' }} />}
    {unavailable.length > 0 && <div className="location-note">
      <span className="truncate-1">{unavailable.length === 1 ? `Couldn’t load ${unavailable[0].name}’s locations` : `Couldn’t load locations from ${unavailable.length} friends`}</span>
      <InfoButton label="Details" icon={<Info />} width={280}>
        <h4>Couldn’t load locations</h4>
        <dl className="location-errors">{unavailable.map(f => <div key={f.id}><dt>{f.name}</dt><dd>{errors[f.id].replace(/^Error:\s*/, '')}</dd></div>)}</dl>
      </InfoButton>
    </div>}

    <SyncedFolders shared={shared} />
    {!IS_WINDOWS && <SharedFromThisDevice />}
  </div>
}
