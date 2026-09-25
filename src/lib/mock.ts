// Dev-only mock backend. Activates ONLY when the app runs outside Tauri (i.e. a
// plain browser, for UI preview/development). In the real packaged app the Tauri
// APIs are present and this module is never used, so it can't affect production.

import type {
  ChatMessage,
  ChatOverview,
  GifMeta,
  FolderStatus,
  Friend,
  HistoryEntry,
  HistoryItem,
  Pair,
  PairUpdate,
  Settings,
  TransferUpdate,
  VerifyReport,
  VerifyResult,
} from './api'
import type { SyncedFolder as MockSyncedFolder, SyncedFolderStatus as MockSyncedFolderStatus } from './api'

type Cb = (payload: unknown) => void
const buses: Record<string, Set<Cb>> = {}

export function emit(event: string, payload: unknown) {
  buses[event]?.forEach((cb) => cb(payload))
}

export function mockListen(event: string, cb: Cb): Promise<() => void> {
  ;(buses[event] ??= new Set()).add(cb)
  return Promise.resolve(() => {
    buses[event]?.delete(cb)
  })
}

let blocked: { id: string; name: string; at: number; endpointIds: string[] }[] = [
  { id: 'mock-blocked-1', name: 'Spam Account', at: Date.now() - 3 * 86_400_000, endpointIds: ['mock-blocked-1'] },
]

let settings: Settings = {
  downloadDir: '/Users/you/Downloads',
  displayName: "Ashton's MacBook Pro",
  theme: 'system',
  minimizeToTray: true,
  launchAtLogin: true,
  preferDirectP2p: true,
  customRelay: '',
  customRelayPass: '',
  notifyOnComplete: true,
  playSounds: true,
  directMode: false,
  uploadLimitMbps: 0,
  showMegabits: false,
  requireDirect: false,
  waitForDirect: false,
  parallelStreams: true,
  avatar: '',
  notifyOnMessage: true,
  sendReadReceipts: true,
  giphyApiKey: '',
  verboseLogging: false,
  showSyncPopup: true,
  shareDiagnostics: true,
  diagnosticsUrl: '',
  labModeEnabled: false,
  labOperatorId: '',
  folderHistoryKeepDays: 30,
  folderHistoryBudgetBytes: 2 * 1024 * 1024 * 1024,
}

// Preview knobs (dev only): ?empty=1 renders every list empty so the empty
// states can be reviewed; otherwise the preview is seeded with a varied, realistic
// data set (long names, offline friends, relay vs local, failures…).
const EMPTY = typeof location !== 'undefined' && new URLSearchParams(location.search).has('empty')
const MIN = 60_000
const HOUR = 3_600_000
const DAY = 86_400_000
const T0 = Date.now()
const okDigest = 'a3f1c9e2b47d05886e1f2a9c3b4d5e6f708192a3b4c5d6e7f8091a2b3c4d5e6f7'

const history: HistoryEntry[] = EMPTY ? [] : [
  { id: 'h0', direction: 'receive', fileNames: ['IMG_0421.HEIC', 'IMG_0422.HEIC', 'IMG_0423.HEIC'], bytesTotal: 42_000_000, peer: 'Alex', locality: 'local', code: null, state: 'completed', timestampMs: T0 - 12 * MIN, error: null, outDir: '/Users/you/Downloads',
    integrity: [{ name: 'IMG_0421.HEIC', size: 14_000_000, algorithm: 'blake3', digest: okDigest, peerDigest: okDigest, verified: true, acknowledged: true }] },
  { id: 'h1', direction: 'receive', fileNames: ['Vacation Photos.zip'], bytesTotal: 248_000_000, peer: '192.168.1.40:5', locality: 'local', code: null, state: 'completed', timestampMs: T0 - 42 * MIN, error: null, outDir: '/Users/you/Downloads' },
  { id: 'hx-video', direction: 'send', fileNames: ['Clip.mov'], bytesTotal: 128_400_000, peer: 'Alex', locality: 'local', code: null, state: 'completed', timestampMs: T0 - 50 * MIN, error: null, outDir: null },
  { id: 'h2', direction: 'send', fileNames: ['budget-2026.xlsx'], bytesTotal: 84_000, peer: '70.2.1.9:5', locality: 'internet', code: null, state: 'completed', timestampMs: T0 - 8 * HOUR, error: null, outDir: null },
  { id: 'h3', direction: 'send', fileNames: ['demo-reel.mov', 'notes.txt'], bytesTotal: 1_240_000_000, peer: null, locality: 'unknown', code: null, state: 'failed', timestampMs: T0 - 26 * HOUR, error: 'The other side went offline before the transfer finished.', outDir: null },
  { id: 'h4', direction: 'send', fileNames: ['Family Videos — Summer at the lake house (the long edit, final final v3).mov'], bytesTotal: 14_200_000_000, peer: 'Jordan Kim', locality: 'internet', code: null, state: 'completed', timestampMs: T0 - 30 * HOUR, error: null, outDir: null },
  { id: 'h5', direction: 'receive', fileNames: ['Rechnungen_Übersicht_März.pdf'], bytesTotal: 842_000, peer: '陈伟', locality: 'direct', code: null, state: 'completed', timestampMs: T0 - 3 * DAY, error: null, outDir: '/Users/you/Downloads' },
  { id: 'h6', direction: 'receive', fileNames: ['Keynote Deck.key'], bytesTotal: 92_000_000, peer: 'Sam', locality: 'internet', code: null, state: 'canceled', timestampMs: T0 - 4 * DAY, error: null, outDir: null },
  { id: 'h7', direction: 'send', fileNames: ['Screenshot 2026-09-12 at 10.41.22.png'], bytesTotal: 2_100_000, peer: 'Lab Peer', locality: 'direct', code: null, state: 'failed', timestampMs: T0 - 12 * DAY, error: 'Verification failed — the copy on the other device didn’t match.', outDir: null },
  { id: 'h8', direction: 'receive', fileNames: ['song.m4a'], bytesTotal: 7_400_000, peer: 'Priya Raman', locality: 'local', code: null, state: 'completed', timestampMs: T0 - 40 * DAY, error: null, outDir: '/Users/you/Downloads' },
]

let counter = 0
// Pending manual-accept offers awaiting respondToOffer (mock only).
const pendingOffers: Record<string, TransferUpdate> = {}
// In-memory chat threads keyed by friend id (mock only).
const mockChats: Record<string, ChatMessage[]> = {}
function seedChats() {
  if (EMPTY) return
  let seq = 0
  const msg = (peerId: string, ago: number, fromMe: boolean, text: string, extra: Partial<ChatMessage> = {}): ChatMessage => ({
    id: `seed-${peerId}-${++seq}`, peerId, fromMe, kind: 'text', text, files: [], bytes: 0, path: null,
    status: fromMe ? 'read' : null, ts: T0 - ago, seq, reactions: [], edited: false, deleted: false, gif: null, ...extra,
  })
  const file = (peerId: string, ago: number, fromMe: boolean, files: string[], bytes: number, path: string | null, extra: Partial<ChatMessage> = {}) =>
    msg(peerId, ago, fromMe, '', { kind: 'file', files, bytes, path, ...extra })
  mockChats.f1 = [
    msg('f1', 2 * DAY, false, 'Hey! Did the footage come through?'),
    msg('f1', 2 * DAY - MIN, true, 'Half of it — the relay was crawling. Trying again on the same Wi-Fi now'),
    msg('f1', 3 * HOUR, false, 'Perfect. Here’s the link to the brief: https://example.com/brief/a-very-long-path-that-should-wrap-nicely-in-the-bubble', { reactions: [{ emoji: '👍', fromMe: true }] }),
    msg('f1', 3 * HOUR - MIN, true, 'Got it', { replyTo: 'seed-f1-3', replyPreview: 'Here’s the link to the brief…', edited: true }),
    file('f1', 2 * HOUR, false, ['Beach.jpg'], 3_400_000, '/mock-media/beach.jpg', { reactions: [{ emoji: '❤️', fromMe: true }, { emoji: '❤️', fromMe: false }] }),
    file('f1', 2 * HOUR - MIN, false, ['Portrait.jpg'], 2_100_000, '/mock-media/portrait.jpg'),
    file('f1', 2 * HOUR - 2 * MIN, false, ['Q3 Report.pdf'], 18_400_000, '/Users/you/Downloads/Q3 Report.pdf'),
    msg('f1', 90 * MIN, true, 'This message was unsent', { deleted: true }),
    file('f1', 55 * MIN, true, ['Clip.mov'], 128_400_000, '/mock-media/clip.webm', { status: 'delivered', fileXferId: 'hx-video' }),
    file('f1', 50 * MIN, true, ['design-system.fig', 'tokens.json', 'README.md', 'logo.svg'], 58_000_000, '/Users/you/Desktop/design-system.fig', { status: 'delivered' }),
    file('f1', 40 * MIN, true, ['Presentation.key'], 312_000_000, '/Users/you/Desktop/Presentation.key', { status: 'delivered', fileXferId: 'cx-failed', fileXferFailed: true }),
    msg('f1', 30 * MIN, false, 'مرحبا — سلام — こんにちは — Ünïcödé names work too'),
    file('f1', 6 * MIN, true, ['raw-footage-day2.mov'], 4_800_000_000, '/Users/you/Movies/raw-footage-day2.mov', { status: 'sending', fileXferId: 'cx-live' }),
    msg('f1', 5 * MIN, true, 'Sending the rest tonight.', { status: 'delivered' }),
  ]
  mockChats.f3 = [
    msg('f3', 26 * HOUR, false, 'Can you drop the brand files in the shared folder?'),
    msg('f3', 25 * HOUR, true, 'On it'),
    msg('f3', 20 * MIN, false, 'Thanks!! Also — lunch Thursday?'),
    msg('f3', 19 * MIN, false, 'I’m buying'),
  ]
  mockChats.f6 = [file('f6', 3 * DAY, false, ['Mountains.jpg'], 1_900_000, '/mock-media/mountains.jpg')]
  mockChats.f4 = [msg('f4', 9 * DAY, true, 'Welcome aboard!', { status: 'delivered' })]
}
seedChats()
// Shared-folder activity woven into Alex's thread (store reads this key at start).
if (typeof localStorage !== 'undefined' && !EMPTY && !localStorage.getItem('dropbeam-folder-activity-v2')) {
  try {
    localStorage.setItem('dropbeam-folder-activity-v2', JSON.stringify({ p1: [
      { id: 'fa1', ts: T0 - 100 * MIN, direction: 'receive', files: ['Moodboard/ref-01.jpg', 'Moodboard/ref-02.jpg', 'Moodboard/ref-03.jpg'], bytes: 9_400_000, pairId: 'p1', from: 'Alex' },
      { id: 'fa2', ts: T0 - 35 * MIN, direction: 'send', files: ['cover-photo.png'], bytes: 2_200_000, pairId: 'p1' },
      { id: 'fa3', ts: T0 - 20 * MIN, direction: 'send', files: [], bytes: 0, pairId: 'p1', action: 'moved', moves: [{ from: 'drafts/brief-v2.pdf', to: 'final/brief-v2.pdf' }] },
    ] }))
  } catch { /* preview only */ }
}
if (typeof localStorage !== 'undefined' && !EMPTY && !localStorage.getItem('dropbeam-chat-unread')) {
  try { localStorage.setItem('dropbeam-chat-unread', JSON.stringify({ f3: 2, f6: 14 })) } catch { /* preview only */ }
}

const pairBase = { secret: 'mock', autoDelete: false, deleteMode: 'trash' as const, endpointId: null, groupId: null }
let pairs: Pair[] = EMPTY ? [] : [
  // Mock: we own p1 (matches myEndpointId below) so the owner role controls render.
  { ...pairBase, id: 'p1', role: 'a', peerName: 'Alex', folder: '/Users/you/Desktop/Project (shared with Alex)', twoWay: true, mirror: true, createdAt: T0 - 3 * DAY, ownerEid: 'mock-me' },
  { ...pairBase, id: 'p2', role: 'a', peerName: 'Sam', folder: '/Users/you/Pictures/Family Photos', twoWay: true, mirror: true, createdAt: T0 - 20 * DAY, ownerEid: 'mock-me' },
  { ...pairBase, id: 'p3', role: 'b', peerName: 'Jordan Kim', folder: '/Users/you/Documents/Design Assets — Brand Refresh 2026 (Final Deliverables)', twoWay: false, mirror: false, createdAt: T0 - 9 * DAY, iAmViewer: true, ownerEid: 'mock-jordan' },
  { ...pairBase, id: 'p4', role: 'a', peerName: 'Priya Raman', folder: '/Users/you/Documents/Tax Documents 2026', twoWay: true, mirror: true, createdAt: T0 - 60 * DAY, ownerEid: 'mock-me' },
  { ...pairBase, id: 'p5', role: 'b', peerName: 'Chen Wei', folder: '/Users/you/Music/Band Practice', twoWay: true, mirror: false, createdAt: T0 - 90 * DAY, ownerEid: 'mock-chen' },
  { ...pairBase, id: 'p6', role: 'a', peerName: '', folder: '/Users/you/Desktop/Wedding Plans', twoWay: true, mirror: true, createdAt: T0 - 2 * HOUR, ownerEid: 'mock-me' },
]
let pairCounter = 6

const folderHistory: Record<string, HistoryItem[]> = EMPTY ? {} : {
  p1: [
    { id: 'fh1', relPath: 'src/old-logo.svg', size: 24_000, reason: 'deleted', timestampMs: T0 - 36 * MIN },
    { id: 'fh2', relPath: 'notes.md', size: 4_200, reason: 'replaced', timestampMs: T0 - 5 * HOUR },
    { id: 'fh3', relPath: 'drafts/v1.fig', size: 742_000_000, reason: 'deleted', timestampMs: T0 - 2 * DAY },
    { id: 'fh4', relPath: 'shoot/raw/IMG_0421.CR2', size: 38_400_000, reason: 'deleted', timestampMs: T0 - 9 * DAY },
  ],
  p2: [
    { id: 'fh5', relPath: '2025/Christmas/IMG_2231 — the one where everyone is actually looking at the camera.jpg', size: 6_200_000, reason: 'deleted', timestampMs: T0 - 3 * HOUR },
    { id: 'fh6', relPath: '2025/Christmas/IMG_2232.jpg', size: 5_900_000, reason: 'replaced', timestampMs: T0 - 3 * HOUR },
  ],
}

let friends: Friend[] = EMPTY ? [] : [
  { id: 'f1', role: 'a', name: 'Alex', secret: 'mock', createdAt: T0 - 5 * DAY, autoAccept: true, endpointId: 'mock-endpoint-alex', avatar: null, deviceKind: 'laptop', deviceOs: 'macos' },
  { id: 'f2', role: 'b', name: 'Sam', secret: 'mock', createdAt: T0 - 2 * DAY, autoAccept: false, endpointId: null, avatar: null, deviceKind: 'desktop', deviceOs: 'windows' },
  { id: 'f3', role: 'a', name: 'Jordan Kim', secret: 'mock', createdAt: T0 - 9 * DAY, autoAccept: true, endpointId: 'mock-jordan', avatar: null, deviceKind: 'laptop', deviceOs: 'linux' },
  { id: 'f4', role: 'b', name: 'Maximilian Alexander von Hohenzollern-Sigmaringen', secret: 'mock', createdAt: T0 - 1 * DAY, autoAccept: true, endpointId: null, avatar: null },
  { id: 'f5', role: 'a', name: 'Priya Raman', secret: 'mock', createdAt: T0 - 60 * DAY, autoAccept: true, endpointId: 'mock-priya', avatar: null, deviceKind: 'phone', deviceOs: 'ios' },
  { id: 'f6', role: 'a', name: 'Chen Wei', secret: 'mock', createdAt: T0 - 90 * DAY, autoAccept: true, endpointId: 'mock-chen', avatar: null, deviceKind: 'desktop', deviceOs: 'linux' },
  { id: 'f7', role: 'a', name: 'Lab Peer', secret: 'mock', createdAt: T0 - 4 * HOUR, autoAccept: true, endpointId: 'mock-lab', avatar: null, deviceKind: 'laptop', deviceOs: 'macos' },
  { id: 'f8', role: 'b', name: 'Mom', secret: 'mock', createdAt: T0 - 200 * DAY, autoAccept: false, endpointId: 'mock-mom', avatar: null, deviceKind: 'phone', deviceOs: 'ios' },
  // One of MY devices (same account) — rendered under "My devices", not Friends.
  { id: 'mock-phone', role: 'a', name: 'iPhone', secret: 'mock', createdAt: T0 - 30 * DAY, autoAccept: true, endpointId: 'preview-phone', avatar: null, deviceKind: 'phone', deviceOs: 'ios', accountPub: 'preview-account' },
]
/** Who the preview treats as reachable, and how. Everyone else is offline. */
const ONLINE: Record<string, { path: string; rttMs: number; upgrading: boolean; relay: string | null }> = {
  f1: { path: 'local', rttMs: 3, upgrading: false, relay: null },
  f3: { path: 'relay', rttMs: 88, upgrading: true, relay: 'use1' },
  f6: { path: 'direct', rttMs: 41, upgrading: false, relay: null },
  f7: { path: 'direct', rttMs: 14, upgrading: false, relay: null },
  'mock-phone': { path: 'local', rttMs: 6, upgrading: false, relay: null },
}
// Seed "last seen" for a few offline friends so every presence label shows.
if (typeof localStorage !== 'undefined' && !EMPTY && !localStorage.getItem('dropbeam-friend-seen')) {
  try {
    localStorage.setItem('dropbeam-friend-seen', JSON.stringify({ sam: T0 - 3 * HOUR, 'priya raman': T0 - 2 * DAY, mom: T0 - 9 * DAY }))
  } catch { /* preview only */ }
}
let friendCounter = 8

function base(id: string, direction: 'send' | 'receive', names: string[]): TransferUpdate {
  return {
    id,
    direction,
    state: 'starting',
    code: null,
    fileNames: names,
    fileCount: names.length,
    percent: 0,
    bytesDone: 0,
    bytesTotal: 0,
    speedBps: 0,
    etaSeconds: null,
    locality: 'unknown',
    peer: null,
    error: null,
    outDir: direction === 'receive' ? settings.downloadDir : null,
    friendName: null,
  }
}

/** Simulated transfers still ticking, so the dev mock can pause one mid-flight. */
const running = new Map<string, { t: TransferUpdate; iv: ReturnType<typeof setInterval> }>()

/** Completed simulated transfers, so the preview can run "Verify copy" on one. */
const finished = new Map<string, TransferUpdate>()
const verifying = new Map<string, ReturnType<typeof setInterval>>()

function simulate(t: TransferUpdate, total: number) {
  t.bytesTotal = total
  t.peer = '192.168.1.55:51022'
  t.locality = 'local'
  let pct = 0
  const iv: ReturnType<typeof setInterval> = setInterval(() => {
    pct += 6 + Math.random() * 9
    if (pct >= 100) {
      t.state = 'transferring'
      t.percent = 100
      t.bytesDone = total
      t.speedBps = 44_000_000
      t.etaSeconds = 0
      emit('transfer://update', { ...t })
      clearInterval(iv)
      running.delete(t.id)
      setTimeout(() => {
        t.state = 'completed'
        finished.set(t.id, t)
        emit('transfer://update', { ...t })
        history.unshift({
          id: t.id,
          direction: t.direction,
          fileNames: t.fileNames,
          bytesTotal: total,
          peer: t.peer,
          locality: t.locality,
          code: t.code,
          state: 'completed',
          timestampMs: Date.now(),
          error: null,
          outDir: t.outDir,
        })
        emit('history://changed', null)
      }, 600)
      return
    }
    t.state = 'transferring'
    t.percent = pct
    t.bytesDone = Math.round((total * pct) / 100)
    t.speedBps = 36_000_000 + Math.random() * 16_000_000
    t.etaSeconds = (100 - pct) / 11
    emit('transfer://update', { ...t })
  }, 550)
  running.set(t.id, { t, iv })
}

// Dev helper to preview incoming transfers: window.__mockIncoming(true) for a
// manual-accept offer, false for an auto receive that streams progress.
function mockIncoming(manual: boolean) {
  const id = `r${++counter}`
  const t = base(id, 'receive', manual ? ['Q3 Report.pdf'] : [])
  t.friendName = 'Alex'
  t.bytesTotal = 18_400_000
  if (manual) {
    t.state = 'waitingForAccept'
    pendingOffers[id] = t
    emit('transfer://update', { ...t })
  } else {
    t.state = 'connecting'
    emit('transfer://update', { ...t })
    setTimeout(() => simulate(t, 64_000_000), 900)
  }
}
// Dev helper to preview the outgoing transfer card: window.__mockSend('Hui').
function mockSend(to = 'Hui') {
  const id = `s${++counter}`
  const t = base(id, 'send', ['Voice Over Main (Fixed).txt'])
  t.friendName = to
  t.state = 'connecting'
  emit('transfer://update', { ...t })
  setTimeout(() => simulate(t, 2_400_000), 700)
}
// Dev helper for the UI sweep: window.__mockGallery() drops one transfer card in
// EVERY state (plus long names), so the whole design language can be reviewed
// without a second device. (Chats are seeded at startup — see seedChats.)
function mockGallery() {
  const mk = (id: string, dir: 'send' | 'receive', names: string[], patch: Partial<TransferUpdate>) => {
    const t = { ...base(id, dir, names), bytesTotal: 248_000_000, peer: '192.168.1.55:51022', ...patch }
    emit('transfer://update', t)
  }
  mk('g1', 'send', ['Vacation Photos 2026 — the full, unedited, extremely long album name that keeps going.zip'], { state: 'transferring', percent: 42, bytesDone: 104_000_000, speedBps: 38_000_000, etaSeconds: 38, locality: 'direct', friendName: 'Alex', connDetail: { path: 'direct', rttMs: 14, upgrading: false, relay: null } })
  mk('g2', 'receive', ['Q3 Report.pdf'], { state: 'waitingForAccept', friendName: 'Sam', bytesTotal: 18_400_000 })
  mk('g3', 'send', ['design-system.fig', 'tokens.json', 'README.md'], { state: 'connecting', friendName: 'Sam', fileCount: 3 })
  mk('g4', 'send', ['raw-footage.mov'], { state: 'transferring', percent: 8, bytesDone: 1_000_000_000, bytesTotal: 12_400_000_000, speedBps: 2_100_000, etaSeconds: 5400, locality: 'internet', friendName: 'Alex', connDetail: { path: 'relay', rttMs: 88, upgrading: true, relay: 'use1' }, detail: 'Waiting for a direct connection' })
  mk('g5', 'send', ['Budget 2026.xlsx'], { state: 'paused', percent: 61, bytesDone: 5_100_000, bytesTotal: 8_400_000, friendName: 'Alex' })
  mk('g6', 'send', ['Presentation.key'], { state: 'failed', percent: 23, error: 'The other side went offline before the transfer finished.', friendName: 'Sam' })
  mk('g7', 'receive', ['IMG_0421.HEIC', 'IMG_0422.HEIC', 'IMG_0423.HEIC', 'IMG_0424.HEIC'], { state: 'completed', percent: 100, bytesDone: 42_000_000, bytesTotal: 42_000_000, fileCount: 4, locality: 'local', friendName: 'Alex', outDir: settings.downloadDir })
  mk('g8', 'send', ['notes.txt'], { state: 'canceled', percent: 12, friendName: 'Sam' })
  mk('g9', 'send', ['Quick send bundle.zip'], { state: 'waitingForPeer', code: 'dropbeam:MOCKquicksendcode0000aaaabbbbccccdddd', percent: 0 })
}
// Live chat-file transfers for the seeded Alex thread: one mid-flight send and one
// that failed, so the in-chat progress and Retry states render in the preview.
if (typeof window !== 'undefined' && !EMPTY) {
  setTimeout(() => {
    emit('transfer://update', { ...base('cx-live', 'send', ['raw-footage-day2.mov']), state: 'transferring', friendName: 'Alex', bytesTotal: 4_800_000_000, bytesDone: 1_920_000_000, percent: 40, speedBps: 88_000_000, etaSeconds: 33, locality: 'local', peer: '192.168.1.40:5', connDetail: { path: 'local', rttMs: 3, upgrading: false, relay: null }, chatTransfer: { id: 'cx-live', offset: 0, total: 4_800_000_000, last: true } })
    emit('transfer://update', { ...base('cx-failed', 'send', ['Presentation.key']), state: 'failed', friendName: 'Alex', bytesTotal: 312_000_000, bytesDone: 71_000_000, percent: 23, error: 'Alex went offline before the file finished.', chatTransfer: { id: 'cx-failed', offset: 0, total: 312_000_000, last: true } })
  }, 600)
}
if (typeof window !== 'undefined') {
  const w = window as unknown as { __mockIncoming?: (m: boolean) => void; __mockSend?: (to?: string) => void }
  w.__mockIncoming = mockIncoming
  w.__mockSend = mockSend
  ;(window as unknown as { __mockGallery?: () => void }).__mockGallery = mockGallery
  ;(window as unknown as { __mockEmit?: typeof emit }).__mockEmit = emit
}

// ── Synced folders (dev preview) ─────────────────────────────────────────────
// Enough behaviour to exercise every card state: a healthy folder, one whose
// host is asleep, and whatever the preview adds.
let mockFolders: MockSyncedFolder[] = EMPTY ? [] : [
  {
    id: 'sf1', friendId: 'f1', locationId: 'loc1', relPath: 'Travel',
    localPath: '/Users/you/Pictures/Travel', enabled: true, deleteRemote: false,
    createdAt: Date.now() - 86_400_000, lastCheckAt: Date.now() - 120_000,
    lastResult: { ok: true, message: 'Up to date' },
  },
  {
    id: 'sf2', friendId: 'f1', locationId: 'loc1', relPath: '',
    localPath: '/Users/you/Documents/Scans', enabled: false, deleteRemote: true,
    createdAt: Date.now() - 400_000_000, lastCheckAt: Date.now() - 7_200_000,
    lastResult: { ok: true, message: 'Up to date' },
  },
]
let mockStatuses: Record<string, MockSyncedFolderStatus> = {
  sf1: { id: 'sf1', state: 'idle', pendingFiles: 0, lastCheckAt: Date.now() - 120_000, message: 'Up to date', transferId: null },
  sf2: { id: 'sf2', state: 'paused', pendingFiles: 0, lastCheckAt: Date.now() - 7_200_000, message: 'Paused — nothing is being copied', transferId: null },
}
function pushStatus(id: string, patch: Partial<MockSyncedFolderStatus>) {
  const next = { ...mockStatuses[id], ...patch, id } as MockSyncedFolderStatus
  mockStatuses = { ...mockStatuses, [id]: next }
  emit('location-sync://status', next)
}
/** Folders friends share with this device, so the preview can exercise the picker. */
export const mockSharedLocations = async (friendId: string) =>
  EMPTY ? [] : friendId === 'f1'
    ? [
        { id: 'loc1', name: 'Buddy NAS', rights: { upload: true, manage: true }, reachable: true, freeBytes: 1_320_000_000_000, totalBytes: 4_000_000_000_000 },
        { id: 'loc2', name: 'Alex Photo Archive', rights: { upload: false, manage: false }, reachable: true, freeBytes: null, totalBytes: null },
      ]
    : friendId === 'f6'
      ? [{ id: 'loc3', name: 'Studio Scratch Disk — Projects 2024–2026', rights: { upload: true, manage: false }, reachable: false }]
      : friendId === 'f3' ? Promise.reject(new Error('Jordan’s DropBeam is too old to share Locations.')) : []

/** Folders THIS device hosts (Settings → Locations, and the gateway cards). */
let mockHosted = EMPTY ? [] : [
  { id: 'h-nas', name: 'Buddy NAS', path: '/Volumes/buddy/Shared', friendIds: ['f1', 'f6', 'f3'], rights: { upload: true, manage: false } },
  { id: 'h-ext', name: 'Photo Backup', path: '/Volumes/T7 Shield/Photo Backup', friendIds: [], rights: { upload: false, manage: false } },
]
export const mockLocations = {
  listHosted: async () => mockHosted,
  hostedStatus: async (id: string) => id === 'h-nas'
    ? { id, reachable: true, freeBytes: 1_320_000_000_000, markerOk: true, error: null, lastActivity: { at: T0 - 25 * MIN, friendId: 'f1', direction: 'upload', bytes: 412_000_000 } }
    : { id, reachable: false, freeBytes: 0, markerOk: false, error: 'The disk isn’t connected.', lastActivity: null },
  save: async (loc: { id: string }) => { mockHosted = [...mockHosted.filter((l) => l.id !== loc.id), loc as typeof mockHosted[number]]; return mockHosted },
  remove: async (id: string) => { mockHosted = mockHosted.filter((l) => l.id !== id); return mockHosted },
  activity: async () => EMPTY ? [] : [
    { friendId: 'f1', locationId: 'h-nas', operation: 'upload', item: 'Taxes/2026/W-2.pdf', at: T0 - 25 * MIN },
    { friendId: 'f6', locationId: 'h-nas', operation: 'download', item: 'Mixes/Final Master v12.wav', at: T0 - 3 * HOUR },
    { friendId: 'f1', locationId: 'h-nas', operation: 'rename', item: 'Old name.txt', to: 'New name.txt', at: T0 - 2 * DAY },
  ],
  mountCandidates: async () => [
    { label: 'buddy', path: '/Volumes/buddy', fstype: 'smbfs', kind: 'network', freeBytes: 1_320_000_000_000, totalBytes: 4_000_000_000_000 },
    { label: 'T7 Shield', path: '/Volumes/T7 Shield', fstype: 'apfs', kind: 'removable', freeBytes: 210_000_000_000, totalBytes: 1_000_000_000_000 },
  ],
}

export const mockSyncedFolders = {
  list: async (): Promise<MockSyncedFolder[]> => mockFolders,
  statuses: async (): Promise<Record<string, MockSyncedFolderStatus>> => mockStatuses,
  add: async (friendId: string, locationId: string, relPath: string, localPath: string, deleteRemote: boolean) => {
    const id = `synced-${++counter}`
    mockFolders = [...mockFolders, { id, friendId, locationId, relPath, localPath, enabled: true, deleteRemote, createdAt: Date.now(), lastCheckAt: 0, lastResult: null }]
    pushStatus(id, { state: 'scanning', pendingFiles: 0, lastCheckAt: 0, message: 'Checking this folder…', transferId: null })
    setTimeout(() => pushStatus(id, { state: 'uploading', pendingFiles: 12, message: 'Copying to Buddy NAS…' }), 900)
    setTimeout(() => pushStatus(id, { state: 'idle', pendingFiles: 0, lastCheckAt: Date.now(), message: 'Up to date' }), 3200)
    return mockFolders
  },
  update: async (id: string, changes: { enabled?: boolean; deleteRemote?: boolean }) => {
    mockFolders = mockFolders.map((f) => (f.id === id ? { ...f, ...changes } : f))
    if (changes.enabled === false) pushStatus(id, { state: 'paused', pendingFiles: 0, message: 'Paused — nothing is being copied' })
    if (changes.enabled === true) pushStatus(id, { state: 'scanning', message: 'Checking this folder…' })
    return mockFolders
  },
  remove: async (id: string) => {
    mockFolders = mockFolders.filter((f) => f.id !== id)
    return mockFolders
  },
  syncNow: async (id: string) => {
    pushStatus(id, { state: 'scanning', message: 'Checking this folder…' })
    setTimeout(() => pushStatus(id, { state: 'waiting', message: 'Waiting for Linux Box' }), 1200)
    setTimeout(() => pushStatus(id, { state: 'idle', lastCheckAt: Date.now(), message: 'Up to date' }), 3600)
  },
}

function mockFolderStatus(p: Pair): FolderStatus {
  const idle: FolderStatus = {
    pairId: p.id, state: 'idle', queued: 0, sendingFile: null, percent: 0, bytesDone: 0, bytesTotal: 0,
    speedBps: 0, etaSeconds: null, detail: null, peerOnline: !!p.peerName, peerName: p.peerName || null, locality: 'unknown', peerFiles: 1284,
  }
  switch (p.id) {
    case 'p1': return { ...idle, state: 'sending', queued: 3, queuedFiles: ['shoot/IMG_0425.CR2', 'shoot/IMG_0426.CR2', 'shoot/IMG_0427 — alternate angle, slightly out of focus.CR2'], sendingFile: 'beach-sunset.jpg', percent: 62, bytesDone: 77_000_000, bytesTotal: 124_000_000, speedBps: 41_000_000, etaSeconds: 1.1, locality: 'local', sessionTotalFiles: 12, sessionDoneFiles: 8, connDetail: { path: 'local', rttMs: 3, upgrading: false, relay: null } }
    case 'p2': return { ...idle, state: 'waiting', queued: 14, peerOnline: false, detail: 'Waiting for Sam' }
    case 'p3': return { ...idle, state: 'receiving', sendingFile: 'Logo — primary lockup (dark).svg', percent: 18, bytesDone: 18_000_000, bytesTotal: 98_000_000, speedBps: 2_100_000, etaSeconds: 38, locality: 'internet', connDetail: { path: 'relay', rttMs: 88, upgrading: true, relay: 'use1' } }
    case 'p4': return { ...idle, paused: true, peerOnline: false }
    case 'p5': return { ...idle, peerUnshared: true, peerOnline: true, locality: 'direct' }
    case 'p6': return { ...idle, peerOnline: false }
    default: return idle
  }
}

export const mockApi = {
  clearTransferCache: async (): Promise<number> => 0,
  setCardActive: async (): Promise<void> => {},
  frontendLog: async (): Promise<void> => {},
  takeLaunchFile: async (): Promise<string | null> => null,
  verifyFolders: async (): Promise<void> => {},
  verifyFolder: async (): Promise<VerifyResult> => {
    // Simulate the manifest round-trip taking a moment, then report a match.
    await new Promise((r) => setTimeout(r, 1400))
    return {
      peerOnline: true,
      compared: true,
      identical: true,
      matched: 1234,
      differences: 0,
      missingOnPeer: 0,
      missingLocally: 0,
      pendingDeletes: 0,
      localFiles: 1234,
      peerFiles: 1234,
    }
  },
  stopFolderTransfer: async (): Promise<void> => {},
  setFolderPaused: async (): Promise<void> => {},
  sendFiles: async (paths: string[]): Promise<TransferUpdate> => {
    const id = `m${++counter}`
    const names = paths.map((p) => p.split('/').pop() || p)
    const t = base(id, 'send', names)
    setTimeout(() => {
      t.state = 'waitingForPeer'
      t.code = `${4000 + Math.floor(Math.random() * 5000)}-mizar-cobalt`
      emit('transfer://update', { ...t })
      setTimeout(() => simulate(t, 124_000_000), 3200)
    }, 250)
    return base(id, 'send', names)
  },
  receiveFiles: async (_code: string): Promise<TransferUpdate> => {
    const id = `m${++counter}`
    const t = base(id, 'receive', [])
    setTimeout(() => {
      t.state = 'connecting'
      emit('transfer://update', { ...t })
      setTimeout(() => {
        t.fileNames = ['project-export.bin']
        t.fileCount = 1
        simulate(t, 64_000_000)
      }, 1400)
    }, 300)
    return base(id, 'receive', [])
  },
  irohSend: async (paths: string[]): Promise<TransferUpdate> => {
    const id = `m${++counter}`
    const names = paths.map((p) => p.split('/').pop() || p)
    const t = base(id, 'send', names)
    setTimeout(() => {
      t.state = 'waitingForPeer'
      // Realistic length (address JSON, ~390 chars) so previews show a true-density QR.
      t.code = 'directeyJhZGRyIjp7ImlkIjoiMGYxZTJkM2M0YjVhNjk3ODg3OTZhNWI0YzNkMmUxZjAwZjFlMmQzYzRiNWE2OTc4ODc5NmE1YjRjM2QyZTFmMCIsImFkZHJzIjpbeyJSZWxheSI6Imh0dHBzOi8vdXNlMS0xLnJlbGF5Lm4wLmlyb2gubGluay4vIn0seyJJcCI6IjE5Mi4xNjguMS4yMzo1MjAxMSJ9LHsiSXAiOiIyMDMuMC4xMTMuNDQ6NTIwMTEifSx7IklwIjoiWzIwMDE6ZGI4OjNlNDA6NWMxMDo6MmJdOjUyMDExIn1dfSwidG9rZW4iOiI3ZjNjOWExZS01YjJkLTRjOGYtOWU2MS0wYTRkMmI3YzhlMTMifQ'
      emit('transfer://update', { ...t })
      // Preview: ?holdCode=1 keeps the card waiting so the QR can be inspected.
      if (!new URLSearchParams(location.search).has('holdCode')) setTimeout(() => simulate(t, 540_000_000), 2600)
    }, 250)
    return base(id, 'send', names)
  },
  irohReceive: async (_ticket: string): Promise<TransferUpdate> => {
    const id = `m${++counter}`
    const t = base(id, 'receive', [])
    setTimeout(() => {
      t.state = 'connecting'
      emit('transfer://update', { ...t })
      setTimeout(() => {
        t.fileNames = ['project-export.bin']
        t.fileCount = 1
        simulate(t, 540_000_000)
      }, 900)
    }, 300)
    return base(id, 'receive', [])
  },
  irohSelftest: async (): Promise<string> => 'ok · node a1b2c3…f7e8',
  cancelTransfer: async (_id: string): Promise<void> => {},
  pauseTransfer: async (id: string): Promise<void> => {
    const live = running.get(id)
    if (!live) return
    clearInterval(live.iv)
    running.delete(id)
    live.t.state = 'paused'
    live.t.speedBps = 0
    live.t.etaSeconds = null
    live.t.detail = 'Paused — resume any time'
    emit('transfer://update', { ...live.t })
  },
  verifyTransfer: async (id: string): Promise<void> => {
    const t = finished.get(id)
    if (!t || verifying.has(id)) return
    const total = Math.max(t.fileCount, 1)
    const bytesTotal = t.bytesTotal || 1
    const report = (state: VerifyReport['state'], checked: number): VerifyReport => ({
      state,
      checked,
      total,
      bytesHashed: Math.round((bytesTotal * checked) / total),
      bytesTotal,
      mismatched: [],
      missing: [],
      error: null,
    })
    let checked = 0
    t.verify = report('running', 0)
    emit('transfer://update', { ...t })
    const iv = setInterval(() => {
      checked = Math.min(total, checked + Math.max(1, Math.ceil(total / 8)))
      const done = checked >= total
      if (done) {
        clearInterval(iv)
        verifying.delete(id)
      }
      t.verify = report(done ? 'done' : 'running', checked)
      emit('transfer://update', { ...t })
    }, 500)
    verifying.set(id, iv)
  },
  cancelVerify: async (id: string): Promise<void> => {
    const iv = verifying.get(id)
    const t = finished.get(id)
    if (!iv || !t?.verify) return
    clearInterval(iv)
    verifying.delete(id)
    t.verify = { ...t.verify, state: 'canceled' }
    emit('transfer://update', { ...t })
  },
  getSettings: async (): Promise<Settings> => settings,
  updateSettings: async (s: Settings): Promise<Settings> => {
    settings = s
    return s
  },
  getHistory: async (): Promise<HistoryEntry[]> => [...history],
  clearHistory: async (): Promise<void> => {
    history.length = 0
  },
  removeHistoryEntry: async (id: string): Promise<void> => {
    const i = history.findIndex((h) => h.id === id)
    if (i >= 0) history.splice(i, 1)
  },
  setProfileAvatar: async (): Promise<Settings> => settings,
  clearProfileAvatar: async (): Promise<Settings> => {
    settings = { ...settings, avatar: '' }
    return settings
  },
  pickPhotos: async (): Promise<string[]> => ['/demo/Beach.jpg', '/demo/Clip.mov'],
  shareFiles: async (_paths: string[]): Promise<void> => {},
  pickFiles: async (): Promise<string[]> => [
    '/Users/you/Desktop/Q3 Presentation.key',
    '/Users/you/Desktop/cover-photo.png',
  ],
  pickDirectory: async (): Promise<string | null> => '/Users/you/Desktop/Beam to Alex',
  revealPath: async (_path: string): Promise<void> => {},
  openPath: async (_path: string): Promise<void> => {},
  exportDiagnostics: async (): Promise<string> => '/Users/you/Downloads/DropBeam-diagnostics-0.txt',
  diagnosticsTest: async (): Promise<string> => 'Sent a test digest (3 distinct issues) to your endpoint.',
  restartApp: async (): Promise<void> => {},
  lanNetworkBlocked: async (): Promise<boolean> => typeof location !== 'undefined' && new URLSearchParams(location.search).has('lan'),
  openLocalNetworkSettings: async (): Promise<void> => {},
  openUrl: async (_url: string): Promise<void> => {},
  getDefaultDownloadDir: async (): Promise<string> => '/Users/you/Downloads',

  createPair: async (
    folder: string,
    twoWay: boolean,
    peerName?: string,
    mirror?: boolean,
  ): Promise<{ pair: Pair; invite: string }> => {
    const id = `p${++pairCounter}`
    const pair: Pair = {
      id,
      role: 'a',
      peerName: peerName?.trim() || '',
      secret: 'mock',
      folder,
      twoWay: twoWay || !!mirror,
      mirror: !!mirror,
      autoDelete: false,
      deleteMode: 'trash',
      createdAt: Date.now(),
      endpointId: null,
      groupId: null,
    }
    pairs.push(pair)
    if (peerName?.trim()) {
      friends.push({
        id: `f${++friendCounter}`,
        role: 'a',
        name: peerName.trim(),
        secret: 'mock',
        createdAt: Date.now(),
        autoAccept: true,
        endpointId: null,
        avatar: null,
      })
    }
    return { pair, invite: `dropbeam1:MOCK${id}invitecodewouldgohere0000` }
  },
  acceptPair: async (_invite: string, folder: string): Promise<Pair> => {
    const id = `p${++pairCounter}`
    const pair: Pair = {
      id,
      role: 'b',
      peerName: 'Sam',
      secret: 'mock',
      folder,
      twoWay: true,
      mirror: false,
      autoDelete: false,
      deleteMode: 'trash',
      createdAt: Date.now(),
      endpointId: null,
      groupId: null,
    }
    pairs.push(pair)
    return pair
  },
  listPairs: async (): Promise<Pair[]> => [...pairs],
  updatePair: async (u: PairUpdate): Promise<Pair> => {
    const p = pairs.find((x) => x.id === u.id)!
    if (u.twoWay != null) p.twoWay = u.twoWay
    if (u.mirror != null) {
      p.mirror = u.mirror
      if (u.mirror) {
        p.twoWay = true
        p.autoDelete = false
      }
    }
    if (u.autoDelete != null) p.autoDelete = u.autoDelete
    if (u.deleteMode) p.deleteMode = u.deleteMode
    if (u.peerName) p.peerName = u.peerName
    return { ...p }
  },
  removePair: async (id: string): Promise<void> => {
    pairs = pairs.filter((p) => p.id !== id)
  },
  setMemberRole: async (id: string, viewer: boolean): Promise<void> => {
    pairs = pairs.map((p) => (p.id === id ? { ...p, peerIsViewer: viewer } : p))
  },
  myEndpointId: async (): Promise<string | null> => 'mock-me',
  pairInvite: async (id: string): Promise<string> => `dropbeam1:MOCK${id}invitecodewouldgohere0000`,
  folderAddPerson: async (id: string): Promise<string> => `dropbeam1:MOCK${id}groupinvitewouldgohere000`,
  inviteFriendToFolder: async (
    _pairId: string,
    _friendId: string,
    _code?: string | null,
  ): Promise<void> => {},
  listFolderHistory: async (pairId: string): Promise<HistoryItem[]> =>
    [...(folderHistory[pairId] ?? [])].sort((a, b) => b.timestampMs - a.timestampMs),
  restoreFolderItem: async (pairId: string, itemId: string): Promise<void> => {
    folderHistory[pairId] = (folderHistory[pairId] ?? []).filter((i) => i.id !== itemId)
  },
  forgetFolderItem: async (pairId: string, itemId: string): Promise<void> => {
    folderHistory[pairId] = (folderHistory[pairId] ?? []).filter((i) => i.id !== itemId)
  },
  folderHistorySummary: async () =>
    Object.entries(folderHistory)
      .map(([pairId, items]) => {
        const pair = pairs.find((p) => p.id === pairId)
        const folder = pair?.folder ?? pairId
        return {
          pairId,
          folderName: folder.split('/').filter(Boolean).pop() ?? folder,
          folder,
          bytes: items.reduce((s, i) => s + i.size, 0),
          itemCount: items.length,
          oldestMs: items.length ? Math.min(...items.map((i) => i.timestampMs)) : null,
        }
      })
      .filter((s) => s.itemCount > 0),
  clearFolderHistory: async (pairId: string): Promise<number> => {
    const freed = (folderHistory[pairId] ?? []).reduce((s, i) => s + i.size, 0)
    folderHistory[pairId] = []
    return freed
  },
  clearAllFolderHistory: async (): Promise<number> => {
    let freed = 0
    for (const k of Object.keys(folderHistory)) {
      freed += (folderHistory[k] ?? []).reduce((s, i) => s + i.size, 0)
      folderHistory[k] = []
    }
    return freed
  },
  getFolderStatuses: async (): Promise<FolderStatus[]> => pairs.map((p) => mockFolderStatus(p)),

  createFriend: async (friendName: string): Promise<{ friend: Friend; invite: string }> => {
    const id = `f${++friendCounter}`
    const friend: Friend = {
      id,
      role: 'a',
      name: friendName.trim() || 'New friend',
      secret: 'mock',
      createdAt: Date.now(),
      autoAccept: true,
      endpointId: null,
      avatar: null,
    }
    friends.push(friend)
    return { friend, invite: `dropbeamf1:MOCK${id}friendinvitewouldgohere0000` }
  },
  acceptFriend: async (_invite: string): Promise<Friend> => {
    const id = `f${++friendCounter}`
    const friend: Friend = {
      id,
      role: 'b',
      name: 'Jordan',
      secret: 'mock',
      createdAt: Date.now(),
      autoAccept: true,
      endpointId: null,
      avatar: null,
    }
    friends.push(friend)
    return friend
  },
  listFriends: async (): Promise<Friend[]> => [...friends],
  renameFriend: async (id: string, name: string): Promise<void> => {
    const f = friends.find((x) => x.id === id)
    if (f && name.trim()) f.name = name.trim()
  },
  removeFriend: async (id: string): Promise<void> => {
    friends = friends.filter((f) => f.id !== id)
  },
  blockFriend: async (id: string): Promise<string[]> => {
    const f = friends.find((x) => x.id === id)
    if (!f) throw new Error('That friend no longer exists.')
    const eid = f.endpointId || `mock-${f.id}`
    blocked = [{ id: eid, name: f.name, at: Date.now(), endpointIds: [eid] }, ...blocked.filter((b) => b.id !== eid)]
    friends = friends.filter((x) => x.id !== id)
    setTimeout(() => emit('blocked://changed', null), 0)
    return [eid]
  },
  unblockPerson: async (id: string): Promise<void> => {
    blocked = blocked.filter((b) => !b.endpointIds.includes(id))
    setTimeout(() => emit('blocked://changed', null), 0)
  },
  listBlocked: async () => [...blocked],
  openMailto: async (url: string): Promise<void> => {
    console.info('[mock] open mail:', url)
    ;(window as unknown as { __lastMailto?: string }).__lastMailto = url
  },
  setFriendAutoAccept: async (id: string, autoAccept: boolean): Promise<void> => {
    const f = friends.find((x) => x.id === id)
    if (f) f.autoAccept = autoAccept
  },
  pingFriend: async (id: string): Promise<boolean> => {
    await new Promise((r) => setTimeout(r, 1200))
    return !!ONLINE[id]
  },
  probeConnection: async (friendId: string) => {
    await new Promise((r) => setTimeout(r, 600))
    return ONLINE[friendId] ?? null
  },
  forceRelay: async (): Promise<void> => {},
  respondToOffer: async (id: string, accept: boolean): Promise<void> => {
    const t = pendingOffers[id]
    if (!t) return
    delete pendingOffers[id]
    if (!accept) {
      t.state = 'canceled'
      emit('transfer://update', { ...t })
      return
    }
    simulate(t, t.bytesTotal || 64_000_000)
  },
  friendInvite: async (id: string): Promise<string> =>
    `dropbeamf1:MOCK${id}friendinvitewouldgohere0000`,
  myInviteCode: async (): Promise<string> => 'dropbeam:MOCKpersonalcodewouldgohere0000',
  addFriendByCode: async (code: string): Promise<Friend> => {
    const f: Friend = {
      id: `f${++counter}`,
      role: 'b',
      name: 'New friend',
      secret: '',
      createdAt: Date.now(),
      autoAccept: true,
      endpointId: code.slice(0, 12),
      avatar: null,
    }
    friends.push(f)
    return f
  },
  macosInstallHint: async (): Promise<string | null> => null,
  sendToFriend: async (id: string, paths: string[]): Promise<TransferUpdate> => {
    const friend = friends.find((f) => f.id === id)
    const tid = `m${++counter}`
    const names = paths.map((p) => p.split('/').pop() || p)
    const t = base(tid, 'send', names)
    t.friendName = friend?.name ?? 'Friend'
    setTimeout(() => {
      t.state = 'connecting'
      t.friendName = friend?.name ?? 'Friend'
      emit('transfer://update', { ...t })
      setTimeout(() => {
        t.friendName = friend?.name ?? 'Friend'
        simulate(t, 88_000_000)
      }, 1200)
    }, 250)
    const initial = base(tid, 'send', names)
    initial.friendName = friend?.name ?? 'Friend'
    return initial
  },
  getChatMessages: async (friendId: string): Promise<ChatMessage[]> =>
    [...(mockChats[friendId] ?? [])],
  listChats: async (): Promise<ChatOverview[]> =>
    Object.entries(mockChats)
      .map(([peerId, msgs]) => {
        const last = msgs[msgs.length - 1]
        return {
          peerId,
          lastText: !last ? '' : last.deleted ? 'Message deleted' : last.gif ? '🎞️ GIF' : last.kind === 'file' ? (last.files.length === 1 ? `📎 ${last.files[0]}` : `📎 ${last.files.length} files`) : last.text,
          lastTs: last?.ts ?? 0,
          lastFromMe: last?.fromMe ?? false,
          count: msgs.length,
        }
      })
      .sort((a, b) => b.lastTs - a.lastTs),
  sendChatMessage: async (
    friendId: string,
    text: string,
    replyTo?: string | null,
    replyPreview?: string | null,
  ): Promise<ChatMessage> => {
    const m: ChatMessage = {
      id: `c${++counter}`,
      peerId: friendId,
      fromMe: true,
      kind: 'text',
      text,
      files: [],
      bytes: 0,
      path: null,
      status: 'delivered',
      ts: Date.now(),
      seq: counter,
      replyTo: replyTo ?? null,
      replyPreview: replyPreview ?? null,
      reactions: [],
      edited: false,
      deleted: false,
      gif: null,
    }
    ;(mockChats[friendId] ??= []).push(m)
    emit('chat://message', m)
    return m
  },
  sendChatFileNote: async (
    friendId: string,
    names: string[],
    bytes: number,
    paths: string[],
    caption = '',
  ): Promise<ChatMessage> => {
    const m: ChatMessage = {
      id: `c${++counter}`,
      peerId: friendId,
      fromMe: true,
      kind: 'file',
      text: caption,
      files: names,
      bytes,
      path: paths[0] ?? null,
      status: 'delivered',
      ts: Date.now(),
      seq: counter,
      replyTo: null,
      replyPreview: null,
      reactions: [],
      edited: false,
      deleted: false,
      gif: null,
    }
    ;(mockChats[friendId] ??= []).push(m)
    emit('chat://message', m)
    return m
  },
  pasteClipboardImage: async (): Promise<string> => { throw new Error('No usable image is on the clipboard.') },
  savePastedImage: async (_b64: string, ext: string): Promise<string> =>
    `/tmp/mock-pasted.${ext}`,
  reactToMessage: async (friendId: string, messageId: string, emoji: string, add: boolean) => {
    const thread = mockChats[friendId] ?? []
    const i = thread.findIndex((x) => x.id === messageId)
    if (i < 0) return
    const m = { ...thread[i], reactions: [...thread[i].reactions] }
    const ri = m.reactions.findIndex((r) => r.fromMe && r.emoji === emoji)
    if (add && ri < 0) m.reactions.push({ emoji, fromMe: true })
    else if (!add && ri >= 0) m.reactions.splice(ri, 1)
    thread[i] = m
    emit('chat://message', m)
  },
  editChatMessage: async (friendId: string, messageId: string, text: string) => {
    const thread = mockChats[friendId] ?? []
    const i = thread.findIndex((x) => x.id === messageId)
    if (i < 0) return
    const m = { ...thread[i], text, edited: true }
    thread[i] = m
    emit('chat://message', m)
  },
  deleteChatMessage: async (friendId: string, messageId: string) => {
    const thread = mockChats[friendId] ?? []
    const i = thread.findIndex((x) => x.id === messageId)
    if (i < 0) return
    const m = { ...thread[i], deleted: true, text: '', files: [], path: null, gif: null, reactions: [] }
    thread[i] = m
    emit('chat://message', m)
  },
  sendTyping: async () => {},
  sendReadReceipt: async () => {},
  downloadGif: async (url: string) => url,
  sendChatGif: async (
    friendId: string,
    name: string,
    bytes: number,
    path: string,
    gif: GifMeta,
  ): Promise<ChatMessage> => {
    const m: ChatMessage = {
      id: `c${++counter}`,
      peerId: friendId,
      fromMe: true,
      kind: 'file',
      text: '',
      files: [name],
      bytes,
      path,
      status: 'delivered',
      ts: Date.now(),
      seq: counter,
      replyTo: null,
      replyPreview: null,
      reactions: [],
      edited: false,
      deleted: false,
      gif,
    }
    ;(mockChats[friendId] ??= []).push(m)
    emit('chat://message', m)
    return m
  },
  setActiveChat: async () => {},
  setUnreadBadge: async () => {},
}

/** Dev preview of browsing a friend's Location: a small, varied listing (folders,
 *  long unicode names, big numbers) so the file table can be reviewed. */
export function mockLocationRequest(request: Record<string, unknown>): Promise<unknown> {
  const kind = String(request.kind ?? '')
  if (kind !== 'locations.ls') return Promise.resolve({ ok: true })
  const day = 86_400_000
  let entries = [
    { name: '2026 Photos', isDir: true, size: 0, modified: Date.now() - 2 * day },
    { name: 'Backups', isDir: true, size: 0, modified: Date.now() - 40 * day },
    { name: 'Family Videos — Summer at the lake house (the long edit, final final v3).mov', isDir: false, size: 14_200_000_000, modified: Date.now() - 3 * day },
    { name: 'Rechnungen_Übersicht_März.pdf', isDir: false, size: 842_000, modified: Date.now() - 9 * day },
    { name: 'تقرير-المشروع.docx', isDir: false, size: 120_400, modified: Date.now() - 12 * day },
    { name: 'notes.md', isDir: false, size: 4_100, modified: Date.now() - 3_600_000 },
  ]
  const q = String(request.query ?? '').toLowerCase()
  if (q) entries = entries.filter((e) => e.name.toLowerCase().includes(q))
  return new Promise((r) => setTimeout(() => r({ entries, page: 0, hasMore: false, total: entries.length }), 250))
}
