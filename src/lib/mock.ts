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

const history: HistoryEntry[] = [
  {
    id: 'h1',
    direction: 'receive',
    fileNames: ['Vacation Photos.zip'],
    bytesTotal: 248_000_000,
    peer: '192.168.1.40:5',
    locality: 'local',
    code: null,
    state: 'completed',
    timestampMs: Date.now() - 42 * 60_000,
    error: null,
    outDir: '/Users/you/Downloads',
  },
  {
    id: 'h2',
    direction: 'send',
    fileNames: ['budget-2026.xlsx'],
    bytesTotal: 84_000,
    peer: '70.2.1.9:5',
    locality: 'internet',
    code: null,
    state: 'completed',
    timestampMs: Date.now() - 8 * 3600_000,
    error: null,
    outDir: null,
  },
  {
    id: 'h3',
    direction: 'send',
    fileNames: ['demo-reel.mov', 'notes.txt'],
    bytesTotal: 1_240_000_000,
    peer: null,
    locality: 'unknown',
    code: null,
    state: 'failed',
    timestampMs: Date.now() - 26 * 3600_000,
    error: 'The other side went offline before the transfer finished.',
    outDir: null,
  },
]

let counter = 0
// Pending manual-accept offers awaiting respondToOffer (mock only).
const pendingOffers: Record<string, TransferUpdate> = {}
// In-memory chat threads keyed by friend id (mock only).
const mockChats: Record<string, ChatMessage[]> = {}

let pairs: Pair[] = [
  {
    id: 'p1',
    role: 'a',
    peerName: 'Alex',
    secret: 'mock',
    folder: '/Users/you/Desktop/Project (shared with Alex)',
    twoWay: true,
    mirror: true,
    autoDelete: false,
    deleteMode: 'trash',
    createdAt: Date.now() - 3 * 86400_000,
    endpointId: null,
    groupId: null,
    // Mock: we own this folder (matches myEndpointId below) so the owner role
    // controls render in dev/preview.
    ownerEid: 'mock-me',
  },
]
let pairCounter = 1

const folderHistory: Record<string, HistoryItem[]> = {
  p1: [
    { id: 'fh1', relPath: 'src/old-logo.svg', size: 24_000, reason: 'deleted', timestampMs: Date.now() - 36 * 60_000 },
    { id: 'fh2', relPath: 'notes.md', size: 4_200, reason: 'replaced', timestampMs: Date.now() - 5 * 3600_000 },
    { id: 'fh3', relPath: 'drafts/v1.fig', size: 742_000_000, reason: 'deleted', timestampMs: Date.now() - 2 * 86400_000 },
    { id: 'fh4', relPath: 'shoot/raw/IMG_0421.CR2', size: 38_400_000, reason: 'deleted', timestampMs: Date.now() - 9 * 86400_000 },
  ],
}

let friends: Friend[] = [
  { id: 'f1', role: 'a', name: 'Alex', secret: 'mock', createdAt: Date.now() - 5 * 86400_000, autoAccept: true, endpointId: 'mock-endpoint-alex', avatar: null },
  { id: 'f2', role: 'b', name: 'Sam', secret: 'mock', createdAt: Date.now() - 2 * 86400_000, autoAccept: false, endpointId: null, avatar: null },
]
let friendCounter = 2

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
// EVERY state (plus long names) and seeds a varied chat thread with Alex, so the
// whole design language can be reviewed without a second device.
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
  const now = Date.now()
  const msg = (i: number, fromMe: boolean, text: string, extra: Partial<ChatMessage> = {}): ChatMessage => ({
    id: `gm${i}`, peerId: 'f1', fromMe, kind: 'text', text, files: [], bytes: 0, path: null,
    status: fromMe ? 'read' : null, ts: now - (40 - i) * 60_000, seq: i, reactions: [], edited: false, deleted: false, ...extra,
  })
  const thread: ChatMessage[] = [
    msg(1, false, 'Hey! Did the footage come through?'),
    msg(2, true, 'Half of it — the relay was crawling. Trying again on the same Wi-Fi now 🙌'),
    msg(3, false, 'Perfect. Here’s the link to the brief: https://example.com/brief/a-very-long-path-that-should-wrap-nicely-in-the-bubble', { reactions: [{ emoji: '👍', fromMe: true }] }),
    msg(4, true, 'Got it', { replyTo: 'gm3', replyPreview: 'Here’s the link to the brief…', edited: true }),
    msg(5, false, '', { kind: 'file', files: ['Q3 Report.pdf'], bytes: 18_400_000 }),
    msg(6, true, 'This message was unsent', { deleted: true }),
    msg(7, false, 'مرحبا — سلام — こんにちは — Ünïcödé names work too'),
    msg(8, true, 'Sending the rest tonight.', { status: 'delivered' }),
  ]
  mockChats.f1 = thread
  thread.forEach((m) => emit('chat://message', m))
}
if (typeof window !== 'undefined') {
  const w = window as unknown as { __mockIncoming?: (m: boolean) => void; __mockSend?: (to?: string) => void }
  w.__mockIncoming = mockIncoming
  w.__mockSend = mockSend
  ;(window as unknown as { __mockGallery?: () => void }).__mockGallery = mockGallery
}

// ── Synced folders (dev preview) ─────────────────────────────────────────────
// Enough behaviour to exercise every card state: a healthy folder, one whose
// host is asleep, and whatever the preview adds.
let mockFolders: MockSyncedFolder[] = [
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
/** Folders "Alex" shares with this device, so the preview can exercise the picker. */
export const mockSharedLocations = async (friendId: string) =>
  friendId === 'f1'
    ? [
        { id: 'loc1', name: 'Buddy NAS', rights: { upload: true, manage: true } },
        { id: 'loc2', name: 'Alex Photo Archive', rights: { upload: false, manage: false } },
      ]
    : []

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
  lanNetworkBlocked: async (): Promise<boolean> => false,
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
  getFolderStatuses: async (): Promise<FolderStatus[]> =>
    pairs.map((p) =>
      p.id === 'p1'
        ? {
            pairId: p.id,
            state: 'sending' as const,
            queued: 1,
            sendingFile: 'beach-sunset.jpg',
            percent: 62,
            bytesDone: 77_000_000,
            bytesTotal: 124_000_000,
            speedBps: 41_000_000,
            etaSeconds: 1.1,
            detail: null,
            peerOnline: true,
            peerName: p.peerName || null,
            locality: 'local' as const,
          }
        : {
            pairId: p.id,
            state: 'idle' as const,
            queued: 0,
            sendingFile: null,
            percent: 0,
            bytesDone: 0,
            bytesTotal: 0,
            speedBps: 0,
            etaSeconds: null,
            detail: null,
            peerOnline: !!p.peerName,
            peerName: p.peerName || null,
            locality: 'unknown' as const,
          },
    ),

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
  setFriendAutoAccept: async (id: string, autoAccept: boolean): Promise<void> => {
    const f = friends.find((x) => x.id === id)
    if (f) f.autoAccept = autoAccept
  },
  pingFriend: async (id: string): Promise<boolean> => {
    await new Promise((r) => setTimeout(r, 1200))
    // Alex is "online" in the mock; others aren't.
    const f = friends.find((x) => x.id === id)
    return f?.name === 'Alex'
  },
  probeConnection: async (friendId: string) => {
    await new Promise((r) => setTimeout(r, 600))
    const f = friends.find((x) => x.id === friendId)
    if (f?.name === 'Alex') return { path: 'direct', rttMs: 14, upgrading: false, relay: null }
    return { path: 'relay', rttMs: 48, upgrading: true, relay: 'use1' }
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
          lastText: last ? (last.kind === 'file' ? '📎 File' : last.text) : '',
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
