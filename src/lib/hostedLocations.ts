import type { TransferState, TransferUpdate } from './api.ts'

/**
 * Pure helpers behind "Shared from this device" — the host's view of the folders
 * it hands out. Kept out of the view so they can be tested without a DOM.
 */

/**
 * Only the FIRST receive update of an incoming Location upload carries
 * `locationId` (later progress ticks are minimal snapshots), so the host has to
 * remember which hosted folder a transfer id belongs to — and forget it as soon
 * as the transfer leaves the list, or the map grows for the life of the process.
 * Returns the previous map unchanged when nothing moved, so React can skip the
 * re-render.
 */
export function trackLocationTransfers(
  known: Record<string, string>,
  transfers: Record<string, TransferUpdate>,
): Record<string, string> {
  const next: Record<string, string> = {}
  for (const [id, t] of Object.entries(transfers)) {
    const locationId = known[id] ?? (typeof t.locationId === 'string' ? t.locationId : undefined)
    if (locationId) next[id] = locationId
  }
  const keys = Object.keys(next)
  const same = keys.length === Object.keys(known).length && keys.every((id) => known[id] === next[id])
  return same ? known : next
}

// api.ts exports the same predicate, but importing it at RUNTIME pulls the
// Tauri-only modules in with it. The literal type keeps this honest: a renamed
// or removed transfer state fails the build here too.
const LIVE: readonly TransferState[] =
  ['starting', 'waitingForPeer', 'connecting', 'waitingForAccept', 'transferring']

export interface IncomingToLocation {
  /** Display names of the friends currently pushing, in first-seen order. */
  friends: string[]
  files: number
  bytes: number
}

/**
 * Live inbound uploads grouped by the hosted folder they're landing in, so the
 * card can say "Receiving from Sam · 12 files · 3.4 GB". Only RECEIVES count:
 * a send this device started to someone else's location isn't traffic INTO ours.
 */
export function incomingByLocation(
  transfers: Record<string, TransferUpdate>,
  known: Record<string, string>,
): Record<string, IncomingToLocation> {
  const byLocation: Record<string, IncomingToLocation> = {}
  for (const t of Object.values(transfers)) {
    const locationId = known[t.id]
    if (!locationId || t.direction !== 'receive' || !LIVE.includes(t.state)) continue
    const row = (byLocation[locationId] ??= { friends: [], files: 0, bytes: 0 })
    const who = t.friendName || 'a friend'
    if (!row.friends.includes(who)) row.friends.push(who)
    row.files += t.fileCount || t.fileNames?.length || 0
    row.bytes += t.bytesTotal || 0
  }
  return byLocation
}
