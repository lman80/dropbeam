// Transfer Server: typed bridge + the browser-preview mock.
// Types mirror src-tauri/src/mailbox/{server,client,cmds}.rs (camelCase).

import { useSyncExternalStore } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { HAS_TAURI } from './api'

export type ServerAccess = 'me' | 'chosen' | 'all'

export interface ServerConfig {
  enabled: boolean
  paused: boolean
  name: string
  root: string
  capBytes: number
  fileDays: number
  chatDays: number
  itemMax: number
  access: ServerAccess
  allowed: string[]
  through: string[]
  denied: string[]
  udpPort: number
  /** The account that owns this server when it isn't linked to one ('' = nobody picked). */
  ownerAccount?: string
}

export interface PersonUsage {
  id: string; name: string; own: boolean; items: number; bytes: number; through: boolean
  /** A friend of the owner that the owner's device vouched for (not this computer's friend). */
  viaOwner?: boolean
  /** Removed on this server (can be allowed again). */
  removed?: boolean
}

/** An account that owns (or could own) this server. */
export interface OwnerView { account: string; name: string; devices: number; sharing: number; disputed?: number }
export interface Waiting { label: string; items: number; bytes: number; oldestMs: number; expiresMs: number }

export interface ServerStatus {
  supported: boolean
  config: ServerConfig
  storageOk: boolean
  storageError: string | null
  used: number
  free: number | null
  items: number
  people: PersonUsage[]
  waiting: Waiting[]
  defaultRoot: string
  owner?: OwnerView | null
  /** This computer is linked to an account, so it's the owner's already. */
  linked?: boolean
  ownerChoices?: OwnerView[]
}

export interface DeviceCheck {
  supported: boolean
  os: string
  sleeps: boolean | null
  onBattery: boolean | null
  defaultRoot: string
  freeBytes: number | null
  totalBytes: number | null
  suggestedCap: number
}

export type ServerPatch = Partial<Omit<ServerConfig, 'denied' | 'ownerAccount'>>

/** A Transfer Server this device may use. */
export interface UsableServer {
  eid: string
  name: string
  own: boolean
  member: boolean
  through: boolean
  useIt: boolean
  holdForMe: boolean
  /** 'new' → show the offer card; 'seen' | 'dismissed' | ''. */
  offer: string
  revoked: boolean
  paused: boolean
  learnedMs: number
  /** The server says it's ours (we're one of its owner's devices). */
  owner?: boolean
  /** We let our friends use it. */
  shareFriends?: boolean
  access?: string
  /** Friends' devices that told us about it (their own server). */
  via?: string[]
  /** The thread of the person who shared it, and their name. */
  viaPeer?: string
  viaName?: string
}

export type ServerPrefs = { useIt?: boolean; holdForMe?: boolean; offer?: string; shareFriends?: boolean }

// ── browser-preview mock ──────────────────────────────────────────────────────
const q = typeof location !== 'undefined' ? new URLSearchParams(location.search) : new URLSearchParams()
const GB = 1_000_000_000
const HOUR = 3_600_000
const DAY = 86_400_000
const T0 = Date.now()
let mockCfg: ServerConfig = {
  enabled: q.get('server') !== 'off' && q.get('server') !== 'setup',
  paused: q.get('server') === 'paused',
  name: 'Linux Box',
  root: '/mnt/buddy/DropBeam Transfer',
  capBytes: 200 * GB,
  fileDays: 14,
  chatDays: 30,
  itemMax: 20 * GB,
  access: 'all',
  allowed: [],
  through: ['f6'],
  denied: [],
  udpPort: 0,
}
const mockPeople = (): PersonUsage[] => q.get('server') === 'empty' ? [] : [
  { id: 'own', name: 'You', own: true, items: 3, bytes: 1_200_000_000, through: false },
  { id: 'f1', name: 'Alex', own: false, items: 0, bytes: 0, through: false },
  { id: 'f6', name: 'Chen Wei', own: false, items: 9, bytes: 36_800_000_000, through: true },
  { id: 'v:mock-mong', name: 'Mong', own: false, items: 0, bytes: 0, through: false, viaOwner: true },
]
const mockWaiting = (): Waiting[] => q.get('server') === 'empty' ? [] : [
  { label: 'Alex', items: 2, bytes: 1_100_000_000, oldestMs: T0 - 3 * HOUR, expiresMs: T0 + 13 * DAY },
  { label: 'Someone a friend knows', items: 9, bytes: 36_000_000_000, oldestMs: T0 - 2 * DAY, expiresMs: T0 + 12 * DAY },
]
function mockStatus(): ServerStatus {
  const people = mockCfg.enabled ? mockPeople().filter((p) => !mockCfg.denied?.includes(p.id)) : []
  const used = q.get('server') === 'full' ? mockCfg.capBytes * 0.97 : people.reduce((n, p) => n + p.bytes, 0)
  return {
    supported: q.get('host') !== 'no',
    config: mockCfg,
    storageOk: q.get('server') !== 'unmounted',
    storageError: q.get('server') === 'unmounted' ? 'The storage folder isn’t available (is the drive connected?)' : null,
    used,
    free: 1_800 * GB,
    items: people.reduce((n, p) => n + p.items, 0),
    people,
    waiting: mockCfg.enabled ? mockWaiting() : [],
    defaultRoot: '/Users/you/Library/Application Support/com.dropbeam.app/transfer-server',
    linked: q.get('linked') === '1',
    ownerChoices: [{ account: 'acct-ashton', name: 'Ashton', devices: 2, sharing: 0 }],
    owner: mockCfg.ownerAccount ? { account: mockCfg.ownerAccount, name: 'Ashton', devices: 2, sharing: q.get('sharing') === '0' ? 0 : 1 } : null,
  }
}
let mockServers: UsableServer[] = q.get('servers') === 'none' ? [] : [
  { eid: 'mock-jordan', name: 'Jordan’s Mac mini', own: false, member: true, through: false, useIt: q.get('offer') !== '1', holdForMe: false, offer: q.get('offer') === '1' ? 'new' : 'seen', revoked: false, paused: false, learnedMs: T0 - 5 * DAY },
  ...(q.get('offer') === 'via' ? [{ eid: 'mock-ashbox', name: 'Linux Box', own: false, member: true, through: false, useIt: false, holdForMe: false, offer: 'new', revoked: false, paused: false, learnedMs: T0 - HOUR, via: ['mock-ash'], viaPeer: 'f1', viaName: 'Alex' }] : []),
  ...(q.get('offer') === 'share' ? [{ eid: 'mock-mybox', name: 'Linux Box', own: false, owner: true, member: true, through: true, useIt: true, holdForMe: true, offer: 'share', access: 'all', revoked: false, paused: false, learnedMs: T0 - HOUR }] : []),
  ...(q.get('servers') === 'own' ? [{ eid: 'mock-linux', name: 'Linux Box', own: true, member: true, through: true, useIt: true, holdForMe: true, offer: 'seen', revoked: false, paused: false, learnedMs: T0 - 9 * DAY }] : []),
]
const mockBus = new Set<() => void>()
const ping = () => mockBus.forEach((cb) => cb())

const mock = {
  checkDevice: async (): Promise<DeviceCheck> => ({
    supported: q.get('host') !== 'no', os: 'macos', sleeps: q.get('sleeps') === '1', onBattery: false,
    defaultRoot: '/Users/you/Library/Application Support/com.dropbeam.app/transfer-server',
    freeBytes: 412 * GB, totalBytes: 994 * GB, suggestedCap: 100 * GB,
  }),
  folderSpace: async (path: string): Promise<[number, number] | null> => path.includes('buddy') ? [1_800 * GB, 4_000 * GB] : [412 * GB, 994 * GB],
  status: async (): Promise<ServerStatus> => mockStatus(),
  configure: async (patch: ServerPatch): Promise<ServerStatus> => {
    await new Promise((r) => setTimeout(r, 450))
    mockCfg = { ...mockCfg, ...patch } as ServerConfig
    ping()
    return mockStatus()
  },
  removePerson: async (personId: string): Promise<ServerStatus> => {
    mockCfg = { ...mockCfg, denied: [...mockCfg.denied, personId], through: mockCfg.through.filter((p) => p !== personId) }
    return mockStatus()
  },
  restorePerson: async (personId: string): Promise<ServerStatus> => {
    mockCfg = { ...mockCfg, denied: mockCfg.denied.filter((p) => p !== personId) }
    return mockStatus()
  },
  wipe: async (): Promise<ServerStatus> => mockStatus(),
  setOwner: async (account: string): Promise<ServerStatus> => {
    mockCfg = { ...mockCfg, ownerAccount: account }
    return mockStatus()
  },
  disable: async (): Promise<ServerStatus> => {
    mockCfg = { ...mockCfg, enabled: false }
    return mockStatus()
  },
  servers: async (): Promise<UsableServer[]> => mockServers,
  serverPrefs: async (eid: string, prefs: ServerPrefs): Promise<UsableServer[]> => {
    mockServers = mockServers.map((s) => s.eid === eid ? { ...s, ...prefs } : s)
    ping()
    return mockServers
  },
  forgetServer: async (eid: string): Promise<UsableServer[]> => {
    mockServers = mockServers.filter((s) => s.eid !== eid)
    ping()
    return mockServers
  },
  fetchNow: async (): Promise<void> => {},
  holdRoute: async (friendId: string): Promise<string | null> => q.get('hold') === '0' ? null : ['f2', 'f5', 'f8'].includes(friendId) || q.get('hold') === '1' ? 'Linux Box' : null,
}

export const serverApi = HAS_TAURI ? {
  checkDevice: () => invoke<DeviceCheck>('server_check_device'),
  folderSpace: (path: string) => invoke<[number, number] | null>('server_folder_space', { path }),
  status: () => invoke<ServerStatus>('server_status'),
  configure: (patch: ServerPatch) => invoke<ServerStatus>('server_configure', { patch }),
  removePerson: (personId: string) => invoke<ServerStatus>('server_remove_person', { personId }),
  restorePerson: (personId: string) => invoke<ServerStatus>('server_restore_person', { personId }),
  wipe: () => invoke<ServerStatus>('server_wipe'),
  disable: (deleteItems: boolean) => invoke<ServerStatus>('server_disable', { deleteItems }),
  servers: () => invoke<UsableServer[]>('mailbox_servers'),
  serverPrefs: (eid: string, prefs: ServerPrefs) =>
    invoke<UsableServer[]>('mailbox_server_prefs', { eid, useIt: prefs.useIt ?? null, holdForMe: prefs.holdForMe ?? null, offer: prefs.offer ?? null, shareFriends: prefs.shareFriends ?? null }),
  setOwner: (account: string) => invoke<ServerStatus>('server_set_owner', { account }),
  forgetServer: (eid: string) => invoke<UsableServer[]>('mailbox_forget_server', { eid }),
  fetchNow: () => invoke<void>('mailbox_fetch_now'),
  holdRoute: (friendId: string) => invoke<string | null>('mailbox_hold_route', { friendId }),
} : mock

/** The owner's server changed (items arrived/left, settings saved). */
export function onServerChanged(cb: () => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) {
    mockBus.add(cb)
    return Promise.resolve(() => { mockBus.delete(cb) })
  }
  return listen('mailbox://server', () => cb())
}

/** The servers this device may use changed (a friend shared one, access removed…). */
export function onServersChanged(cb: () => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) {
    mockBus.add(cb)
    return Promise.resolve(() => { mockBus.delete(cb) })
  }
  return listen('mailbox://servers', () => cb())
}

// ── copy helpers (one voice everywhere) ───────────────────────────────────────

/** The short line a chat bubble shows for a server note on an undelivered message. */
export function serverNoteText(note: string | null | undefined, friend: string, server?: string | null): string | null {
  const box = server || 'the Transfer Server'
  switch (note) {
    case 'expired': return `${capital(box)} couldn’t deliver it in time · will send when ${friend} is online`
    case 'lost': case 'refused': return `${capital(box)} couldn’t deliver it · will send when ${friend} is online`
    case 'full': return `${capital(box)} is full · will send when ${friend} is online`
    case 'paused': return `${capital(box)} is paused · will send when ${friend} is online`
    case 'unreachable': return `Couldn’t reach ${box} · will keep trying`
    case 'needs_update': return `${friend} needs to update DropBeam to get messages while offline`
    default: return null
  }
}

const capital = (s: string) => s.charAt(0).toUpperCase() + s.slice(1)

/** "Waiting for 3h", "2 days". */
export function heldFor(since: number, now = Date.now()): string {
  const m = Math.max(0, Math.round((now - since) / 60_000))
  if (m < 60) return m <= 1 ? 'just now' : `${m} min`
  const h = Math.round(m / 60)
  if (h < 36) return `${h}h`
  return `${Math.round(h / 24)} days`
}

/** "in 13 days", "tomorrow", "today". */
export function expiresIn(at: number, now = Date.now()): string {
  const d = Math.floor((at - now) / DAY)
  if (d <= 0) return 'today'
  if (d === 1) return 'tomorrow'
  return `in ${d} days`
}

export const ACCESS_LABEL: Record<ServerAccess, string> = {
  me: 'Only your devices',
  chosen: 'Friends you choose',
  all: 'All your friends',
}

// ── held files from "ask before accepting" friends ────────────────────────────
export interface PendingFile {
  linkId: string
  peerId: string
  server: string
  serverName: string
  itemId: string
  bytes: number
  names: string[]
  at: number
}

let pendingCache: Record<string, PendingFile> = {}
const pendingSubs = new Set<() => void>()
let pendingStarted = false
function refreshPending() {
  const load: Promise<PendingFile[]> = HAS_TAURI
    ? invoke<PendingFile[]>('mailbox_pending_files')
    : Promise.resolve(q.get('pending') === '1' ? [{ linkId: 'receive:mock-chen:held-1', peerId: 'f6', server: 'mock-linux', serverName: 'Linux Box', itemId: 'x', bytes: 48_200_000, names: ['Site photos.zip'], at: T0 - HOUR }] : [])
  load.then((list) => {
    pendingCache = Object.fromEntries(list.map((p) => [p.linkId, p]))
    pendingSubs.forEach((cb) => cb())
  }).catch(() => {})
}
function subscribePending(cb: () => void) {
  pendingSubs.add(cb)
  if (!pendingStarted) {
    pendingStarted = true
    refreshPending()
    if (HAS_TAURI) void listen('mailbox://pending', refreshPending)
  }
  return () => { pendingSubs.delete(cb) }
}
/** The held send behind a received file card that's waiting for your OK. */
export function usePendingFile(linkId: string | null | undefined): PendingFile | undefined {
  const snap = useSyncExternalStore(subscribePending, () => pendingCache)
  return linkId ? snap[linkId] : undefined
}
const DECLINED_KEY = 'dropbeam-declined-held-files'
/** Held sends you declined (so their card says so instead of "on its way"). */
export function isDeclined(linkId: string | null | undefined): boolean {
  if (!linkId) return false
  try { return (JSON.parse(localStorage.getItem(DECLINED_KEY) ?? '[]') as string[]).includes(linkId) } catch { return false }
}
export async function decideFile(linkId: string, accept: boolean): Promise<void> {
  if (HAS_TAURI) await invoke<void>('mailbox_decide_file', { linkId, accept })
  if (!accept) {
    try {
      const list = JSON.parse(localStorage.getItem(DECLINED_KEY) ?? '[]') as string[]
      localStorage.setItem(DECLINED_KEY, JSON.stringify([...list.filter((x) => x !== linkId), linkId].slice(-200)))
    } catch { /* storage unavailable */ }
  }
  const { [linkId]: _, ...rest } = pendingCache
  void _
  pendingCache = rest
  pendingSubs.forEach((cb) => cb())
}
