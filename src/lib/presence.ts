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
