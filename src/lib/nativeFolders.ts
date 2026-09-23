import type { FolderComplete, FolderStatus, Pair } from './api'

/** One member link of a shared folder, as the native Shared Folders screen shows it. */
export interface NativeFolderMember {
  pairId: string
  name: string
  online: boolean
  /** An invite nobody has accepted yet ("Waiting to join…"). */
  pending: boolean
  viewer: boolean
  /** The friend record for this member (matched by device key), when known. */
  friendId: string | null
  /** Only the folder owner may change roles (the engine enforces it too). */
  canSetRole: boolean
}
/** One shared folder (all its pairwise links collapsed), ready to render natively. */
export interface NativeFolder {
  id: string
  /** Any link of the folder — the engine resolves the folder/group from it. */
  pairId: string
  name: string
  path: string
  mode: 'mirror' | 'twoWay' | 'sendOnly' | 'receiveOnly'
  modeLabel: string
  autoDelete: boolean
  iAmViewer: boolean
  iAmOwner: boolean
  paused: boolean
  peerUnshared: boolean
  state: string
  tone: 'ok' | 'busy' | 'warn' | 'error' | 'offline'
  label: string
  percent: number
  bytesDone: number
  bytesTotal: number
  speedBps: number
  etaSeconds: number | null
  currentFile: string | null
  queued: number
  queuedFiles: string[]
  peerFiles: number | null
  inSync: boolean
  locality: string | null
  /** The pending invite link whose code can be shown again (creator only). */
  pendingInvite: string | null
  lastSyncedMs: number | null
  summary: { direction: string; files: number; bytes: number; durationMs: number; avgBps: number } | null
  members: NativeFolderMember[]
}

const baseName = (path: string) => path.replace(/[/\\]+$/, '').split(/[/\\]/).pop() || path
const num = (n: unknown) => (typeof n === 'number' && Number.isFinite(n) ? n : 0)

/** Same wording/priority as the desktop FoldersView status line. */
/** (Swift appends "· synced <relative time>" from lastSyncedMs.) */
export function folderStatusLine(pair: Pair, status: FolderStatus | undefined): { tone: NativeFolder['tone']; label: string } {
  const peer = pair.peerName || 'your friend'
  if (status?.paused) return { tone: 'warn', label: 'Sync paused — Resume to merge changes' }
  if (pair.role === 'a' && !pair.peerName && !status?.peerOnline) return { tone: 'warn', label: 'Waiting for someone to accept the invite' }
  switch (status?.state ?? 'idle') {
    case 'sending': return { tone: 'busy', label: status?.sendingFile ? `Sending ${status.sendingFile}` : 'Sending…' }
    case 'receiving': return { tone: 'busy', label: 'Receiving…' }
    case 'waiting': return { tone: 'warn', label: (status?.detail ?? `Waiting for ${peer}`) + (status && status.queued > 0 ? ` · ${status.queued} queued` : '') }
    case 'error': return { tone: 'error', label: status?.detail ?? 'Something went wrong' }
    default:
      if (status && !status.peerOnline && pair.peerName) return { tone: 'offline', label: `${peer} is offline — will sync when they're back` }
      return { tone: 'ok', label: 'Up to date' }
  }
}

/** Collapse pairwise links into one row per shared folder (group id, else the link itself). */
export function nativeFolders(
  pairs: Pair[],
  statuses: Record<string, FolderStatus>,
  summaries: Record<string, FolderComplete> = {},
  lastSynced: Record<string, number> = {},
  myEid: string | null = null,
  friends: { id: string; endpointId: string | null }[] = [],
): NativeFolder[] {
  const friendByEid = new Map(friends.filter(f => f.endpointId).map(f => [f.endpointId!, f.id]))
  const groups = new Map<string, Pair[]>()
  for (const p of pairs) {
    if (!p || typeof p.id !== 'string') continue
    const key = p.groupId ?? p.id
    groups.set(key, [...(groups.get(key) ?? []), p])
  }
  return [...groups.entries()].map(([id, links]) => {
    const rep = links[0]
    // The folder's live status is the busiest member link's (a group has one per person).
    const rank = (s?: FolderStatus) => (s?.paused ? 5 : s?.state === 'sending' || s?.state === 'receiving' ? 4 : s?.state === 'error' ? 3 : s?.state === 'waiting' ? 2 : s ? 1 : 0)
    const lead = links.reduce((best, p) => (rank(statuses[p.id]) > rank(statuses[best.id]) ? p : best), rep)
    const status = statuses[lead.id]
    const synced = Math.max(0, ...links.map(p => num(lastSynced[p.id])))
    const line = folderStatusLine(lead, status)
    const iAmOwner = !!myEid && !!rep.ownerEid && rep.ownerEid === myEid
    const mode: NativeFolder['mode'] = rep.mirror ? 'mirror' : rep.twoWay ? 'twoWay' : rep.role === 'a' ? 'sendOnly' : 'receiveOnly'
    const summary = links.map(p => summaries[p.id]).find(Boolean)
    const busy = status?.state === 'sending' || status?.state === 'receiving'
    const pending = links.find(p => p.role === 'a' && !p.peerName)
    return {
      id,
      pairId: rep.id,
      name: baseName(rep.folder),
      path: rep.folder,
      mode,
      modeLabel: mode === 'mirror' ? 'Total sync' : mode === 'twoWay' ? 'Two-way' : mode === 'sendOnly' ? 'View only (they receive)' : 'View only (you receive)',
      autoDelete: !!rep.autoDelete && !rep.mirror,
      iAmViewer: !!rep.iAmViewer,
      iAmOwner,
      paused: !!status?.paused || links.some(p => !!statuses[p.id]?.paused),
      peerUnshared: links.every(p => !!statuses[p.id]?.peerUnshared),
      state: status?.state ?? 'idle',
      tone: line.tone,
      label: line.label,
      percent: Math.min(100, Math.max(0, num(status?.percent))),
      bytesDone: num(status?.bytesDone),
      bytesTotal: num(status?.bytesTotal),
      speedBps: num(status?.speedBps),
      etaSeconds: typeof status?.etaSeconds === 'number' && Number.isFinite(status.etaSeconds) ? status.etaSeconds : null,
      currentFile: status?.sendingFile ?? null,
      queued: num(status?.queued),
      queuedFiles: Array.isArray(status?.queuedFiles) ? status.queuedFiles.slice(0, 50) : [],
      peerFiles: typeof status?.peerFiles === 'number' ? status.peerFiles : null,
      inSync: !!rep.mirror && !!status?.peerOnline && status.state === 'idle' && num(status.queued) === 0 && !status.paused,
      locality: busy ? status?.locality ?? null : null,
      pendingInvite: pending?.id ?? null,
      lastSyncedMs: synced || null,
      summary: !busy && summary && summary.files > 0 ? { direction: summary.direction, files: summary.files, bytes: num(summary.bytes), durationMs: num(summary.durationMs), avgBps: num(summary.avgBps) } : null,
      members: links.map(m => ({
        pairId: m.id,
        name: m.peerName || 'Waiting to join…',
        online: !!statuses[m.id]?.peerOnline,
        pending: !m.peerName,
        viewer: !!m.peerIsViewer,
        friendId: (m.endpointId && friendByEid.get(m.endpointId)) || null,
        canSetRole: iAmOwner && !!m.peerName,
      })),
    }
  })
}

/** Every link of one shared folder — leaving must remove each (desktop removeGroup). */
export function folderLinks(pairs: Pair[], folderId: string): Pair[] {
  const links = pairs.filter(p => (p.groupId ?? p.id) === folderId)
  if (!links.length) throw new Error('This shared folder is no longer on this device.')
  return links
}
