import type { ChatMessage, SharedLocation, TransferUpdate } from './api'
import { integrityRows, mergeIntegrity } from './integrity.ts'

const nonnegative = (n: number): number => Number.isFinite(n) && n >= 0 ? n : 0

/**
 * A friend's `locations.list` reply. The host answers in wire (snake_case)
 * form, and a host older than the room report omits `reachable`/`free_bytes`
 * entirely — so read defensively and hand the view ONE shape. An entry without
 * an id or a name is dropped rather than rendered as "undefined".
 */
export function normalizeSharedLocations(raw: unknown): SharedLocation[] {
  if (!Array.isArray(raw)) return []
  const bytes = (n: unknown): number | null =>
    typeof n === 'number' && Number.isFinite(n) && n >= 0 ? n : null
  return raw.flatMap((value) => {
    const l = (value ?? {}) as Record<string, unknown>
    if (typeof l.id !== 'string' || typeof l.name !== 'string') return []
    const rights = (l.rights ?? {}) as Record<string, unknown>
    return [{
      id: l.id,
      name: l.name,
      rights: { upload: rights.upload === true, manage: rights.manage === true },
      reachable: typeof l.reachable === 'boolean' ? l.reachable : undefined,
      freeBytes: bytes(l.free_bytes),
      totalBytes: bytes(l.total_bytes),
    }]
  })
}
const strings = (value: string[]): string[] =>
  Array.isArray(value) ? value.filter((s) => typeof s === 'string') : []

// IPC events and persisted messages can predate fields added to the TS types.
// Normalize at ingestion so every consumer (including previews/search) is safe.
export function normalizeChatMessage(m: ChatMessage): ChatMessage {
  return {
    ...m,
    text: typeof m.text === 'string' ? m.text : '',
    files: strings(m.files),
    reactions: Array.isArray(m.reactions)
      ? m.reactions.filter((r) => r && typeof r.emoji === 'string') : [],
    bytes: nonnegative(m.bytes),
    ts: Number.isFinite(m.ts) && Math.abs(m.ts) <= 8.64e15 ? m.ts : 0,
    seq: nonnegative(m.seq),
  }
}

export function normalizeTransfer(u: TransferUpdate, prev?: TransferUpdate): TransferUpdate {
  // Pause is a waypoint, not an ending: the engine's Paused snapshot is minimal
  // (a staged send never moved a byte, so it carries neither counts nor a
  // recipient), so carry the card's own numbers forward. They're what the Paused
  // badge shows and what Resume replays.
  const held = u.state === 'paused' ? prev : undefined
  const bytesTotal = nonnegative(u.bytesTotal) || nonnegative(held?.bytesTotal ?? 0)
  const bytesDone = nonnegative(u.bytesDone) || nonnegative(held?.bytesDone ?? 0)
  let percent = Number.isFinite(u.percent) ? u.percent
    : bytesTotal > 0 ? bytesDone / bytesTotal * 100 : 0
  if (held && percent === 0 && bytesTotal > 0) percent = bytesDone / bytesTotal * 100
  return {
    ...u,
    // A folder arrives as several pushes on ONE card, each reporting only the
    // files it carried, so the card's checksum list accumulates (a re-verified
    // file replaces its earlier row) instead of being replaced push by push.
    integrity: mergeIntegrity(prev?.integrity, integrityRows(u.integrity)),
    locationSkipped: u.locationSkipped ?? prev?.locationSkipped,
    locationConflicts: u.locationConflicts ?? prev?.locationConflicts,
    locationReplaced: u.locationReplaced ?? prev?.locationReplaced,
    // A verify report outlives the update that carried it: later progress ticks
    // (and any other emit on this card) must not blank the verdict.
    verify: u.verify ?? prev?.verify,
    fileNames: strings(u.fileNames ?? prev?.fileNames ?? []),
    fileCount: nonnegative(u.fileCount) || nonnegative(held?.fileCount ?? 0),
    friendName: u.friendName ?? held?.friendName ?? null,
    bytesTotal,
    bytesDone,
    percent: Math.max(0, Math.min(100, percent)),
    speedBps: nonnegative(u.speedBps),
    etaSeconds: u.etaSeconds != null && Number.isFinite(u.etaSeconds) && u.etaSeconds >= 0
      ? u.etaSeconds : null,
  }
}
