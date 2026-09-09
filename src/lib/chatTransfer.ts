import type { ChatMessage, HistoryEntry, TransferUpdate } from './api'
import { integrityRows, mergeIntegrity } from './integrity.ts'

const terminal = (s: string) => ['completed', 'failed', 'canceled'].includes(s)
export type ChatTransfer = TransferUpdate & { updatedAt?: number; firstSeen?: number; unconfirmed?: boolean }

export function chatTransferLabel(t?: TransferUpdate): string {
  return !t ? 'Waiting for confirmation…'
    : t.state === 'completed' ? (t.direction === 'send' ? 'Delivered' : 'Saved')
    : t.state === 'failed' ? 'Not delivered' : t.state === 'canceled' ? 'Canceled'
    : t.state === 'waitingForAccept' ? 'Waiting for acceptance'
    : t.state === 'transferring' ? (t.direction === 'send' ? 'Sending' : 'Receiving') : 'Connecting…'
}

/** Durable evidence for cards whose live/cache entry is absent. Age alone proves nothing. */
export function restoredChatTransfer(
  m: Pick<ChatMessage, 'fileXferId' | 'fromMe' | 'status' | 'path' | 'files' | 'bytes'>,
  history: HistoryEntry[],
  completedPaths: Record<string, string> = {},
): TransferUpdate | undefined {
  if (!m.fileXferId) return undefined
  const direction = m.fromMe ? 'send' : 'receive'
  const entry = history.filter(h => h.id === m.fileXferId && h.direction === direction && terminal(h.state))
    .sort((a, b) => b.timestampMs - a.timestampMs)[0]
  const confirmed = m.fromMe ? m.status === 'delivered' || m.status === 'read'
    : !!m.path || Object.values(completedPaths).some(Boolean)
  if (!entry && !confirmed) return undefined
  // Explicit terminal history (including verification failure) outranks a chat-note receipt.
  const state = entry?.state ?? 'completed'
  const total = entry?.bytesTotal ?? m.bytes
  return {
    id: m.fileXferId, direction, state,
    fileNames: entry?.fileNames ?? m.files, fileCount: (entry?.fileNames ?? m.files).length,
    bytesTotal: total, bytesDone: state === 'completed' ? total : 0,
    percent: state === 'completed' ? 100 : 0, speedBps: 0, etaSeconds: null,
    locality: entry?.locality ?? 'unknown', integrity: entry?.integrity ?? [],
    code: entry?.code ?? null, peer: entry?.peer ?? null, error: entry?.error ?? null,
    outDir: entry?.outDir ?? null, friendName: null,
  }
}

/** Receiver batch state is certified by the engine, never inferred from last/offset. */
export function chatTransferUpdate(u: TransferUpdate, prev?: TransferUpdate): ChatTransfer {
  const link = u.chatTransfer!
  const attempt = link.attempt ?? 0
  const previous = prev?.chatTransfer?.attempt ?? 0
  if (prev && (attempt < previous || (attempt === previous && terminal(prev.state) && !(prev as ChatTransfer).unconfirmed))) return prev
  const same = attempt === previous ? prev : undefined
  const bytesTotal = link.total
  const bytesDone = Math.max(0, Math.min(bytesTotal, u.direction === 'receive' ? link.bytesDone ?? 0 : u.bytesDone))
  let state = u.direction === 'receive' ? link.batchState ?? (terminal(u.state) && u.state !== 'completed' ? u.state : 'transferring') : u.state
  if (state === 'completed' && bytesDone !== bytesTotal) state = 'transferring'
  return {
    ...u, state, bytesTotal, bytesDone,
    integrity: mergeIntegrity(same?.integrity, u.integrity),
    percent: state === 'completed' ? 100 : bytesTotal > 0 ? Math.min(99, bytesDone / bytesTotal * 100) : 0,
    etaSeconds: u.speedBps > 0 ? (bytesTotal - bytesDone) / u.speedBps : null,
    connDetail: u.connDetail ?? same?.connDetail,
    locality: u.locality === 'unknown' ? same?.locality ?? u.locality : u.locality,
    outDir: u.outDir ?? same?.outDir ?? null,
    updatedAt: Date.now(),
    firstSeen: (prev as ChatTransfer | undefined)?.firstSeen ?? Date.now(),
  }
}

export const CHAT_RECENT_LIMIT = 200
export const CHAT_ORPHAN_TTL = 60_000
/** Called on a maintenance timer, not on progress ticks. Active linked entries survive. */
export function pruneChatTransfers(transfers: Record<string, ChatTransfer>, linked: Set<string>, now = Date.now()) {
  const recent: [string, ChatTransfer][] = []
  for (const [id, t] of Object.entries(transfers)) {
    if (!linked.has(id) && now - (t.firstSeen ?? t.updatedAt ?? 0) >= CHAT_ORPHAN_TTL) delete transfers[id]
    else if (terminal(t.state) || !linked.has(id)) recent.push([id, t])
  }
  recent.sort((a, b) => (b[1].updatedAt ?? 0) - (a[1].updatedAt ?? 0))
  for (const [id] of recent.slice(CHAT_RECENT_LIMIT)) delete transfers[id]
}

const KEY = 'dropbeam-chat-transfer-outcomes'
const record = (v: unknown): v is Record<string, unknown> => !!v && typeof v === 'object' && !Array.isArray(v)
const number = (v: unknown) => typeof v === 'number' && Number.isFinite(v) && v >= 0
const strings = (v: unknown): v is string[] => Array.isArray(v) && v.every(x => typeof x === 'string')
export function loadChatTransfers(): Record<string, ChatTransfer> {
  const out: Record<string, ChatTransfer> = Object.create(null)
  try {
    const raw: unknown = JSON.parse(localStorage.getItem(KEY) ?? '{}')
    if (!record(raw)) return out
    for (const [id, v] of Object.entries(raw)) {
      if (!record(v) || typeof v.id !== 'string' || !['send', 'receive'].includes(String(v.direction))) continue
      const l = v.chatTransfer
      if (!record(l) || l.id !== id || !Number.isSafeInteger(l.attempt) || !number(l.attempt) || !number(l.total)) continue
      if (!['starting', 'waitingForPeer', 'connecting', 'waitingForAccept', 'transferring', 'completed', 'failed', 'canceled'].includes(String(v.state))) continue
      // Old caches cannot prove completion. In-flight attempts restore as interrupted,
      // retaining their own generation instead of resurrecting a previous failure.
      let state = v.state as TransferUpdate['state']
      const unconfirmed = v.unconfirmed === true || !terminal(state) || (state === 'completed' && (v.bytesDone !== l.total || (v.direction === 'receive' && l.batchState !== 'completed')))
      if (unconfirmed) state = 'failed'
      const bytesDone = number(v.bytesDone) ? Math.min(v.bytesDone as number, l.total as number) : 0
      out[id] = {
        integrity: integrityRows(v.integrity),
        id: v.id, direction: v.direction as TransferUpdate['direction'], state,
        chatTransfer: { id, attempt: l.attempt as number, total: l.total as number,
          offset: number(l.offset) ? l.offset as number : 0, last: l.last === true,
          batchState: state, bytesDone, completedFiles: strings(l.completedFiles) ? l.completedFiles : [],
          completedPaths: record(l.completedPaths) ? Object.fromEntries(Object.entries(l.completedPaths).filter(([, p]) => typeof p === 'string')) as Record<string, string> : {},
        },
        bytesDone, bytesTotal: l.total as number, percent: state === 'completed' ? 100 : 0,
        fileNames: strings(v.fileNames) ? v.fileNames : [], fileCount: strings(v.fileNames) ? v.fileNames.length : 0,
        speedBps: 0, etaSeconds: null,
        locality: ['local', 'direct', 'internet'].includes(String(v.locality)) ? v.locality as TransferUpdate['locality'] : 'unknown', connDetail: null,
        code: null, peer: null, friendName: typeof v.friendName === 'string' ? v.friendName : null,
        outDir: typeof v.outDir === 'string' ? v.outDir : null,
        error: state === 'failed' ? (typeof v.error === 'string' && !unconfirmed ? v.error : 'Transfer unconfirmed after restart — retry to confirm.') : null,
        unconfirmed,
        firstSeen: number(v.firstSeen) ? v.firstSeen as number : number(v.updatedAt) ? v.updatedAt as number : 0,
        updatedAt: number(v.updatedAt) ? v.updatedAt as number : 0,
      }
    }
    pruneChatTransfers(out, new Set(Object.keys(out)))
  } catch { /* invalid JSON or unavailable storage */ }
  return out
}
export function saveChatTransfers(transfers: Record<string, TransferUpdate>) {
  try { localStorage.setItem(KEY, JSON.stringify(transfers)) } catch { /* unavailable or full */ }
}

/** Stable manifest keys keep duplicate leaf names independently openable. */
export function completedChatItems(paths: Record<string, string> = {}) {
  return Object.entries(paths).map(([key, path]) => ({
    key, name: key.replace(/^(?:file:\d+:|dir:)/, ''), path,
  }))
}
