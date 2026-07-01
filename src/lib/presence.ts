import type { FolderStatus } from './api'

const ONLINE_WINDOW_MS = 120_000

/**
 * Best-effort "is this friend online right now?" — cheap, no extra network:
 *  - live, if they share a folder whose control channel reports them online;
 *  - else recent, if we transferred with them in the last couple of minutes.
 * Returns null when we simply don't know (no recent contact).
 */
export function friendOnlineState(
  name: string,
  friendSeen: Record<string, number>,
  folderStatuses: Record<string, FolderStatus>,
): boolean | null {
  const key = name.trim().toLowerCase()
  if (!key) return null
  for (const s of Object.values(folderStatuses)) {
    if (s.peerName && s.peerName.trim().toLowerCase() === key) {
      if (s.peerOnline) return true
    }
  }
  const seen = friendSeen[key]
  if (seen && Date.now() - seen < ONLINE_WINDOW_MS) return true
  return null
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
