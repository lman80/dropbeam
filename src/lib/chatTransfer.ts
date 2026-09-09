import type { TransferUpdate } from './api'

/** Project per-push receiver progress into the shared chat batch coordinates. */
export function chatTransferUpdate(u: TransferUpdate, prev?: TransferUpdate): TransferUpdate {
  const link = u.chatTransfer!
  const bytesTotal = link.total
  const bytesDone = Math.min(bytesTotal, link.offset + (u.state === 'completed' ? u.bytesTotal : u.bytesDone))
  const state = u.state === 'completed' && !link.last ? 'transferring' : u.state
  return {
    ...u, state, bytesTotal, bytesDone,
    percent: state === 'completed' ? 100 : bytesTotal > 0 ? bytesDone / bytesTotal * 100 : 0,
    etaSeconds: link.offset > 0 || !link.last
      ? u.speedBps > 0 ? (bytesTotal - bytesDone) / u.speedBps : null : u.etaSeconds,
    connDetail: u.connDetail ?? prev?.connDetail,
    locality: u.locality === 'unknown' ? prev?.locality ?? u.locality : u.locality,
    outDir: u.outDir ?? prev?.outDir ?? null,
  }
}

const KEY = 'dropbeam-chat-transfer-outcomes'
export function loadChatTransfers(): Record<string, TransferUpdate> {
  try { return JSON.parse(localStorage.getItem(KEY) ?? '{}') } catch { return {} }
}
export function saveChatTransfers(transfers: Record<string, TransferUpdate>) {
  try {
    const terminal = Object.entries(transfers).filter(([, t]) => ['completed', 'failed', 'canceled'].includes(t.state))
    localStorage.setItem(KEY, JSON.stringify(Object.fromEntries(terminal.slice(-500))))
  } catch { /* storage unavailable or full */ }
}
