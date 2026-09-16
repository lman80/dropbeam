import type { FolderStatus } from './api'

const ONLINE_WINDOW_MS = 120_000

/**
 * Boolean adapter for the same presence used by Friends, including recent
 * successful probes, transfers, and shared-folder contact in the store.
 * Returns null when we simply don't know (no recent contact).
 */
export function friendOnlineState(
  name: string,
  friendSeen: Record<string, number>,
  folderStatuses: Record<string, FolderStatus>,
): boolean | null {
  const presence = friendPresence(name, friendSeen, folderStatuses)
  return presence.status === 'unknown' ? null : presence.status === 'online'
}

export type PresenceStatus = 'online' | 'offline' | 'unknown'
export interface Presence {
  status: PresenceStatus
  /** When we last had contact (ms), or null if never. */
  lastSeen: number | null
}

/**
 * Richer presence than the boolean above: online / offline / unknown + a last-seen
 * timestamp. "offline" means we've seen them before (or a shared folder reports them
 * offline) but not recently; "unknown" means we've genuinely never had contact.
 */
export function friendPresence(
  name: string,
  friendSeen: Record<string, number>,
  folderStatuses: Record<string, FolderStatus>,
): Presence {
  const key = name.trim().toLowerCase()
  if (!key) return { status: 'unknown', lastSeen: null }
  let folderOnline: boolean | null = null
  for (const s of Object.values(folderStatuses)) {
    if (s.peerName && s.peerName.trim().toLowerCase() === key) {
      if (s.peerOnline) {
        folderOnline = true
        break
      }
      folderOnline = false // we share a folder but their control channel is quiet
    }
  }
  const seen = friendSeen[key] ?? null
  const recentlySeen = seen != null && Date.now() - seen < ONLINE_WINDOW_MS
  if (folderOnline === true || recentlySeen) return { status: 'online', lastSeen: seen }
  if (folderOnline === false || seen != null) return { status: 'offline', lastSeen: seen }
  return { status: 'unknown', lastSeen: null }
}

/** Human label for a presence — "Online now", "Last seen 5m ago", or a send hint. */
export function presenceLabel(p: Presence): string {
  if (p.status === 'online') return 'Online now'
  // Honest: file sends have no store-and-forward — don't promise offline delivery.
  if (p.lastSeen == null) return 'Not seen yet'
  const mins = Math.floor((Date.now() - p.lastSeen) / 60_000)
  if (mins < 1) return 'Active moments ago'
  if (mins < 60) return `Last seen ${mins}m ago`
  const hrs = Math.floor(mins / 60)
  if (hrs < 24) return `Last seen ${hrs}h ago`
  const days = Math.floor(hrs / 24)
  return `Last seen ${days}d ago`
}

/**
 * Presence recovery (#34). Nothing used to re-check a friend who had gone quiet:
 * the control beacon backs off to 60/120/300 s and a friend who came back could
 * read as "offline" until the app was restarted. Any view that shows presence
 * claims a check when it OPENS, and the friend is actively pinged.
 *
 * The cooldown is the safety rail: opening Friends, the Send sheet and Locations
 * in quick succession must not turn into a dial storm, and a friend who really
 * is offline must not be re-dialled on every render.
 */
const RECHECK_COOLDOWN_MS = 20_000
const checkedAt = new Map<string, number>()

/**
 * Friend ids worth actively pinging right now — those with a device address
 * that don't already read as online and haven't been checked in the cooldown.
 * Claiming STAMPS them, so two views opening together only ping once.
 */
export function claimPresenceChecks(
  friends: readonly { id: string; name: string; endpointId?: string | null }[],
  friendSeen: Record<string, number>,
  folderStatuses: Record<string, FolderStatus>,
  now: number = Date.now(),
): string[] {
  const due: string[] = []
  for (const f of friends) {
    if (!f.endpointId) continue // paired pre-Direct-mode: there's nothing to dial
    if (friendPresence(f.name, friendSeen, folderStatuses).status === 'online') continue
    if (now - (checkedAt.get(f.id) ?? -Infinity) < RECHECK_COOLDOWN_MS) continue
    checkedAt.set(f.id, now)
    due.push(f.id)
  }
  // A friend who was removed must not keep a slot in the cooldown map forever
  // (and re-adding them should be checkable at once). Callers always pass the
  // whole friend list, so anything not in it is gone.
  for (const id of [...checkedAt.keys()]) if (!friends.some((f) => f.id === id)) checkedAt.delete(id)
  return due
}

/** Test seam: forget every cooldown stamp. */
export function resetPresenceChecks(): void {
  checkedAt.clear()
}
