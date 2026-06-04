// Dev-only mock backend. Activates ONLY when the app runs outside Tauri (i.e. a
// plain browser, for UI preview/development). In the real packaged app the Tauri
// APIs are present and this module is never used, so it can't affect production.

import type {
  FolderStatus,
  Friend,
  HistoryEntry,
  Pair,
  PairUpdate,
  Settings,
  TransferUpdate,
} from './api'

type Cb = (payload: unknown) => void
const buses: Record<string, Set<Cb>> = {}

function emit(event: string, payload: unknown) {
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
  launchAtLogin: false,
  preferDirectP2p: true,
  customRelay: '',
  customRelayPass: '',
  notifyOnComplete: true,
  playSounds: true,
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

let pairs: Pair[] = [
  {
    id: 'p1',
    role: 'a',
    peerName: 'Alex',
    secret: 'mock',
    folder: '/Users/you/Desktop/Beam to Alex',
    twoWay: true,
    autoDelete: true,
    deleteMode: 'trash',
    createdAt: Date.now() - 3 * 86400_000,
  },
]
let pairCounter = 1

let friends: Friend[] = [
  { id: 'f1', role: 'a', name: 'Alex', secret: 'mock', createdAt: Date.now() - 5 * 86400_000, autoAccept: true },
  { id: 'f2', role: 'b', name: 'Sam', secret: 'mock', createdAt: Date.now() - 2 * 86400_000, autoAccept: false },
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

function simulate(t: TransferUpdate, total: number) {
  t.bytesTotal = total
  t.peer = '192.168.1.55:51022'
  t.locality = 'local'
  let pct = 0
  const iv = setInterval(() => {
    pct += 6 + Math.random() * 9
    if (pct >= 100) {
      t.state = 'transferring'
      t.percent = 100
      t.bytesDone = total
      t.speedBps = 44_000_000
      t.etaSeconds = 0
      emit('transfer://update', { ...t })
      clearInterval(iv)
      setTimeout(() => {
        t.state = 'completed'
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
if (typeof window !== 'undefined') {
  ;(window as unknown as { __mockIncoming?: (m: boolean) => void }).__mockIncoming = mockIncoming
}

export const mockApi = {
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
  cancelTransfer: async (_id: string): Promise<void> => {},
  getSettings: async (): Promise<Settings> => settings,
  updateSettings: async (s: Settings): Promise<Settings> => {
    settings = s
    return s
  },
  getHistory: async (): Promise<HistoryEntry[]> => [...history],
  clearHistory: async (): Promise<void> => {
    history.length = 0
  },
  pickFiles: async (): Promise<string[]> => [
    '/Users/you/Desktop/Q3 Presentation.key',
    '/Users/you/Desktop/cover-photo.png',
  ],
  pickDirectory: async (): Promise<string | null> => '/Users/you/Desktop/Beam to Alex',
  revealPath: async (_path: string): Promise<void> => {},
  openPath: async (_path: string): Promise<void> => {},
  getDefaultDownloadDir: async (): Promise<string> => '/Users/you/Downloads',

  createPair: async (
    folder: string,
    twoWay: boolean,
    peerName?: string,
  ): Promise<{ pair: Pair; invite: string }> => {
    const id = `p${++pairCounter}`
    const pair: Pair = {
      id,
      role: 'a',
      peerName: peerName?.trim() || '',
      secret: 'mock',
      folder,
      twoWay,
      autoDelete: false,
      deleteMode: 'trash',
      createdAt: Date.now(),
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
      autoDelete: false,
      deleteMode: 'trash',
      createdAt: Date.now(),
    }
    pairs.push(pair)
    return pair
  },
  listPairs: async (): Promise<Pair[]> => [...pairs],
  updatePair: async (u: PairUpdate): Promise<Pair> => {
    const p = pairs.find((x) => x.id === u.id)!
    if (u.twoWay != null) p.twoWay = u.twoWay
    if (u.autoDelete != null) p.autoDelete = u.autoDelete
    if (u.deleteMode) p.deleteMode = u.deleteMode
    if (u.peerName) p.peerName = u.peerName
    return { ...p }
  },
  removePair: async (id: string): Promise<void> => {
    pairs = pairs.filter((p) => p.id !== id)
  },
  pairInvite: async (id: string): Promise<string> => `dropbeam1:MOCK${id}invitecodewouldgohere0000`,
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
}
