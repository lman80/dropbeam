import type { TransferState, TransferUpdate } from './api.ts'

/**
 * Uploads THIS device is pushing into a friend's Location (GitHub #30). The
 * engine's send updates don't say which Location they target, so the browser
 * that started one records it here; the Locations views then show it in place
 * instead of only in Send & Receive. Persisted (small, pruned) so a paused
 * upload that survives a restart still shows up where it belongs.
 */
export interface OutgoingLocationUpload {
  friendId: string
  locationId: string
  /** Folder inside the Location the files land in ("" = its top level). */
  relPath: string
}

const KEY = 'dropbeam-location-uploads'
const MAX = 50
let cache: Record<string, OutgoingLocationUpload> | null = null

function load(): Record<string, OutgoingLocationUpload> {
  if (cache) return cache
  cache = {}
  try {
    const raw = JSON.parse(globalThis.localStorage?.getItem(KEY) || '{}') as Record<string, OutgoingLocationUpload>
    for (const [id, u] of Object.entries(raw ?? {})) {
      if (u && typeof u.friendId === 'string' && typeof u.locationId === 'string') {
        cache[id] = { friendId: u.friendId, locationId: u.locationId, relPath: typeof u.relPath === 'string' ? u.relPath : '' }
      }
    }
  } catch { /* unavailable or corrupt storage — start empty */ }
  return cache
}

function save(all: Record<string, OutgoingLocationUpload>) {
  try { globalThis.localStorage?.setItem(KEY, JSON.stringify(all)) } catch { /* best-effort */ }
}

export function trackOutgoingLocationUpload(transferId: string, upload: OutgoingLocationUpload) {
  const all = load()
  all[transferId] = upload
  const ids = Object.keys(all)
  if (ids.length > MAX) for (const id of ids.slice(0, ids.length - MAX)) delete all[id]
  save(all)
}

// Kept literal (not api.ts's isActive) so this stays importable without Tauri.
const SHOWN: readonly TransferState[] =
  ['starting', 'waitingForPeer', 'connecting', 'waitingForAccept', 'transferring', 'paused']

/**
 * The sends into one friend's Location that are still in flight (or paused),
 * oldest first. `known` defaults to the persisted registry; tests pass their own.
 */
export function outgoingForLocation(
  transfers: Record<string, TransferUpdate>,
  friendId: string,
  locationId: string,
  known: Record<string, OutgoingLocationUpload> = load(),
): (TransferUpdate & { relPath: string })[] {
  const out: (TransferUpdate & { relPath: string })[] = []
  for (const [id, u] of Object.entries(known)) {
    const t = transfers[id]
    if (!t || t.direction !== 'send' || !SHOWN.includes(t.state)) continue
    if (u.friendId !== friendId || u.locationId !== locationId) continue
    out.push({ ...t, relPath: u.relPath })
  }
  return out
}

/** Forget uploads whose card is gone (finished and dismissed, or never restored). */
export function pruneOutgoingLocationUploads(transfers: Record<string, TransferUpdate>) {
  const all = load()
  let changed = false
  for (const id of Object.keys(all)) if (!transfers[id]) { delete all[id]; changed = true }
  if (changed) save(all)
}
