import type { SharedLocation } from './api'

export type LocationFriend = { id: string; name: string; endpointId: string | null }
export type LocationLoad = { friendId: string; friendName: string; online: boolean; locations: SharedLocation[]; error: string | null }

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
    try {
      if (!online && options.probe) online = await options.probe(friend.id)
      if (online) locations = await options.list(friend.id)
      else error = `${friend.name} is offline`
    } catch (e) { error = options.errorText?.(friend, e) ?? `${friend.name}: ${e instanceof Error ? e.message : String(e)}` }
    const result = { friendId: friend.id, friendName: friend.name, online, locations, error }
    options.onResult?.(result)
    return result
  }))
}
