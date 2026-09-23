import type { SharedLocation } from './api'

export type LocationFriend = { id: string; name: string; endpointId: string | null }
/** `status` says what the last check found: `ready` (listed; `locations` may be
 * empty = nothing shared with this device), `offline` (unreachable before any
 * request; `locations` is the last known list) or `error` (the request failed). */
export type LocationStatus = 'ready' | 'offline' | 'error'
export type LocationLoad = { friendId: string; friendName: string; online: boolean; locations: SharedLocation[]; error: string | null; status: LocationStatus }

/** Shared per-friend load. Native can probe stale presence; the web view already
 * probes on open and refreshes when presence changes. No UI or store dependency. */
export async function loadLocations(options: {
  friends: LocationFriend[]
  online: (friend: LocationFriend) => boolean
  list: (id: string) => Promise<SharedLocation[]>
  probe?: (id: string) => Promise<boolean>
  cached?: Record<string, SharedLocation[]>
  errorText?: (friend: LocationFriend, error: unknown) => string
  onResult?: (result: LocationLoad) => void
}): Promise<LocationLoad[]> {
  return Promise.all(options.friends.filter(f => f.endpointId).map(async friend => {
    let online = options.online(friend)
    let locations = options.cached?.[friend.id] ?? []
    let error: string | null = null
    let status: LocationStatus = 'ready'
    try {
      if (!online && options.probe) online = await options.probe(friend.id)
      if (online) locations = await options.list(friend.id)
      else { error = `${friend.name} is offline`; status = 'offline' }
    } catch (e) { error = options.errorText?.(friend, e) ?? `${friend.name}: ${e instanceof Error ? e.message : String(e)}`; status = 'error' }
    const result = { friendId: friend.id, friendName: friend.name, online, locations, error, status }
    options.onResult?.(result)
    return result
  }))
}

/** One friend as the native Locations screen draws it. `status` adds two
 * states the loader never returns: `pending` (not checked yet this session)
 * and `unavailable` (no device address, so there is nothing to ask). */
export type NativeLocationRow = Omit<LocationLoad, 'status'> & { status: LocationStatus | 'pending' | 'unavailable'; checking: boolean; checkedAt: number | null }
export type CheckedLoad = LocationLoad & { at: number }
const RECENT_MS = 120_000
export function nativeLocationRows(friends: LocationFriend[], options: {
  presence: (friend: LocationFriend) => boolean
  results: Record<string, CheckedLoad>
  shared: Record<string, SharedLocation[]>
  checking: ReadonlySet<string>
  now: number
}): NativeLocationRow[] {
  return friends.map(f => {
    const last = options.results[f.id]
    const status: NativeLocationRow['status'] = !f.endpointId ? 'unavailable' : last?.status ?? 'pending'
    // A list that just answered IS contact, even before presence catches up.
    const online = options.presence(f) || (last?.status === 'ready' && options.now - last.at < RECENT_MS)
    return {
      friendId: f.id, friendName: f.name, online, status,
      locations: options.shared[f.id] ?? [],
      error: status === 'unavailable' ? 'This friend hasn’t connected from a device yet.' : status === 'error' ? last?.error ?? null : null,
      checking: !!f.endpointId && options.checking.has(f.id),
      checkedAt: last?.at ?? null,
    }
  })
}
