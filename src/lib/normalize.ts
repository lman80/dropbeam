import type { ChatMessage, TransferUpdate } from './api'

const nonnegative = (n: number): number => Number.isFinite(n) && n >= 0 ? n : 0
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
  const bytesTotal = nonnegative(u.bytesTotal)
  const bytesDone = nonnegative(u.bytesDone)
  const percent = Number.isFinite(u.percent) ? u.percent
    : bytesTotal > 0 ? bytesDone / bytesTotal * 100 : 0
  return {
    ...u,
    fileNames: strings(u.fileNames ?? prev?.fileNames ?? []),
    fileCount: nonnegative(u.fileCount),
    bytesTotal,
    bytesDone,
    percent: Math.max(0, Math.min(100, percent)),
    speedBps: nonnegative(u.speedBps),
    etaSeconds: u.etaSeconds != null && Number.isFinite(u.etaSeconds) && u.etaSeconds >= 0
      ? u.etaSeconds : null,
  }
}
