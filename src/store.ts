import { create } from 'zustand'
import {
  api,
  onFolderStatus,
  onHistoryChanged,
  onPairsChanged,
  onTransferUpdate,
  type FolderStatus,
  type Friend,
  type HistoryEntry,
  type Pair,
  type PairUpdate,
  type Settings,
  type TransferUpdate,
} from './lib/api'
import { appVersion, checkUpdate, installUpdate as runInstall } from './lib/updater'
import { playIncoming, playOffer, playReceived, playSent } from './lib/sounds'

interface UpdateState {
  version: string
  notes: string
  installing: boolean
  progress: number
}

export type View = 'send' | 'friends' | 'folders' | 'history' | 'settings'

export interface Toast {
  id: string
  kind: 'info' | 'success' | 'error'
  message: string
}

interface AppStore {
  ready: boolean
  view: View
  settings: Settings | null
  transfers: Record<string, TransferUpdate>
  order: string[]
  dragHovering: boolean
  history: HistoryEntry[]
  pairs: Pair[]
  friends: Friend[]
  folderStatuses: Record<string, FolderStatus>
  toasts: Toast[]
  defaultDownloadDir: string
  appVer: string
  update: UpdateState | null
  checkingUpdate: boolean

  init: () => Promise<void>
  checkForUpdates: (manual: boolean) => Promise<void>
  installUpdate: () => Promise<void>
  setView: (v: View) => void
  setDragHovering: (v: boolean) => void
  applyTheme: (theme: Settings['theme']) => void
  saveSettings: (patch: Partial<Settings>) => Promise<void>
  sendPaths: (paths: string[]) => Promise<void>
  receiveCode: (code: string) => Promise<void>
  upsertTransfer: (u: TransferUpdate) => void
  removeTransfer: (id: string) => void
  reloadHistory: () => Promise<void>
  reloadPairs: () => Promise<void>
  updatePair: (u: PairUpdate) => Promise<void>
  removePair: (id: string) => Promise<void>
  reloadFriends: () => Promise<void>
  sendToFriend: (id: string, paths: string[]) => Promise<void>
  createFriend: (name: string) => Promise<string>
  acceptFriend: (invite: string) => Promise<void>
  renameFriend: (id: string, name: string) => Promise<void>
  removeFriend: (id: string) => Promise<void>
  setFriendAutoAccept: (id: string, autoAccept: boolean) => Promise<void>
  respondToOffer: (id: string, accept: boolean) => Promise<void>
  toast: (kind: Toast['kind'], message: string) => void
  dismissToast: (id: string) => void
}

// Guard against Tauri's occasional double-fire of a single OS file drop.
let lastDropSig = ''
let lastDropAt = 0

export const useStore = create<AppStore>((set, get) => ({
  ready: false,
  view: 'send',
  settings: null,
  transfers: {},
  order: [],
  dragHovering: false,
  history: [],
  pairs: [],
  friends: [],
  folderStatuses: {},
  toasts: [],
  defaultDownloadDir: '',
  appVer: '',
  update: null,
  checkingUpdate: false,

  init: async () => {
    const [settings, history, pairs, friends, statuses, defaultDownloadDir] = await Promise.all([
      api.getSettings(),
      api.getHistory(),
      api.listPairs().catch(() => []),
      api.listFriends().catch(() => []),
      api.getFolderStatuses().catch(() => []),
      api.getDefaultDownloadDir().catch(() => ''),
    ])
    get().applyTheme(settings.theme)
    const folderStatuses: Record<string, FolderStatus> = {}
    statuses.forEach((s) => (folderStatuses[s.pairId] = s))
    set({ settings, history, pairs, friends, folderStatuses, defaultDownloadDir, ready: true })

    // Re-apply theme when the OS appearance changes (only matters for "system").
    window
      .matchMedia('(prefers-color-scheme: dark)')
      .addEventListener('change', () => {
        const s = get().settings
        if (s && s.theme === 'system') get().applyTheme('system')
      })

    onTransferUpdate((u) => get().upsertTransfer(u))
    onHistoryChanged(() => get().reloadHistory())
    onFolderStatus((s) =>
      set((st) => ({ folderStatuses: { ...st.folderStatuses, [s.pairId]: s } })),
    )
    // The control channel can learn the peer's name after the fact — reload pairs
    // so the folder shows who's in it (and clears the stale "waiting" state).
    onPairsChanged(() => get().reloadPairs())

    appVersion().then((v) => set({ appVer: v }))
    // Only the main window owns the update check (the popover/HUD share state but
    // shouldn't each trigger their own launch check).
    const isOverlay =
      typeof document !== 'undefined' &&
      document.documentElement.classList.contains('overlay-window')
    if (!isOverlay) get().checkForUpdates(false)
  },

  checkForUpdates: async (manual) => {
    if (get().checkingUpdate) return
    set({ checkingUpdate: true })
    try {
      const info = await checkUpdate()
      if (info) {
        set({ update: { version: info.version, notes: info.notes, installing: false, progress: 0 } })
        if (manual) get().toast('info', `Update available: v${info.version}`)
      } else if (manual) {
        get().toast('success', "You're on the latest version")
      }
    } catch (e) {
      if (manual) get().toast('error', `Couldn't check for updates: ${e}`)
    } finally {
      set({ checkingUpdate: false })
    }
  },

  installUpdate: async () => {
    const u = get().update
    if (!u) return
    set({ update: { ...u, installing: true, progress: 0 } })
    try {
      await runInstall((pct) => {
        const cur = get().update
        if (cur) set({ update: { ...cur, progress: pct } })
      })
      // The app relaunches inside runInstall on success.
    } catch (e) {
      get().toast('error', `Update failed: ${e}`)
      const cur = get().update
      if (cur) set({ update: { ...cur, installing: false } })
    }
  },

  setView: (view) => set({ view }),

  setDragHovering: (dragHovering) => set({ dragHovering }),

  sendPaths: async (paths) => {
    paths = paths.filter(Boolean)
    if (!paths.length) return
    // De-dupe a double-fired drop of the same paths within 800ms.
    const sig = paths.join('|')
    const now = Date.now()
    if (sig === lastDropSig && now - lastDropAt < 800) return
    lastDropSig = sig
    lastDropAt = now
    set({ view: 'send' })
    try {
      const u = await api.sendFiles(paths)
      get().upsertTransfer(u)
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  receiveCode: async (code) => {
    code = code.trim()
    if (!code) return
    try {
      const u = await api.receiveFiles(code)
      get().upsertTransfer(u)
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  applyTheme: (theme) => {
    const root = document.documentElement
    const sysDark = window.matchMedia('(prefers-color-scheme: dark)').matches
    const dark = theme === 'dark' || (theme === 'system' && sysDark)
    root.classList.toggle('dark', dark)
  },

  saveSettings: async (patch) => {
    const current = get().settings
    if (!current) return
    const next = { ...current, ...patch }
    set({ settings: next })
    if (patch.theme) get().applyTheme(patch.theme)
    try {
      const saved = await api.updateSettings(next)
      set({ settings: saved })
    } catch (e) {
      get().toast('error', `Couldn't save settings: ${e}`)
    }
  },

  upsertTransfer: (u) => {
    const prev = get().transfers[u.id]
    // Sounds fire on meaningful state changes only (not on every progress tick).
    if ((get().settings?.playSounds ?? true) && (!prev || prev.state !== u.state)) {
      if (u.state === 'completed') {
        if (u.direction === 'send') playSent()
        else playReceived()
      } else if (u.state === 'waitingForAccept') {
        playOffer()
      } else if (u.direction === 'receive' && !prev && u.state !== 'failed' && u.state !== 'canceled') {
        // First time we see an incoming transfer (auto-accept) — a soft cue.
        playIncoming()
      }
    }
    set((s) => ({
      transfers: { ...s.transfers, [u.id]: u },
      order: s.order.includes(u.id) ? s.order : [...s.order, u.id],
    }))
  },

  removeTransfer: (id) =>
    set((s) => {
      const next = { ...s.transfers }
      delete next[id]
      return { transfers: next, order: s.order.filter((x) => x !== id) }
    }),

  reloadHistory: async () => {
    const history = await api.getHistory()
    set({ history })
  },

  reloadPairs: async () => {
    const [pairs, statuses] = await Promise.all([
      api.listPairs(),
      api.getFolderStatuses().catch(() => []),
    ])
    const folderStatuses: Record<string, FolderStatus> = {}
    statuses.forEach((s) => (folderStatuses[s.pairId] = s))
    set({ pairs, folderStatuses })
  },

  updatePair: async (u) => {
    try {
      await api.updatePair(u)
      await get().reloadPairs()
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  removePair: async (id) => {
    try {
      await api.removePair(id)
      await get().reloadPairs()
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  reloadFriends: async () => {
    const friends = await api.listFriends().catch(() => [])
    set({ friends })
  },

  sendToFriend: async (id, paths) => {
    paths = paths.filter(Boolean)
    if (!paths.length) return
    set({ view: 'send' })
    try {
      const u = await api.sendToFriend(id, paths)
      get().upsertTransfer(u)
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  createFriend: async (name) => {
    const res = await api.createFriend(name)
    await get().reloadFriends()
    return res.invite
  },

  acceptFriend: async (invite) => {
    const friend = await api.acceptFriend(invite)
    await get().reloadFriends()
    get().toast('success', `You're now friends with ${friend.name}`)
  },

  renameFriend: async (id, name) => {
    try {
      await api.renameFriend(id, name)
      await get().reloadFriends()
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  removeFriend: async (id) => {
    try {
      await api.removeFriend(id)
      await get().reloadFriends()
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  setFriendAutoAccept: async (id, autoAccept) => {
    // Optimistic flip so the toggle feels instant.
    set((s) => ({
      friends: s.friends.map((f) => (f.id === id ? { ...f, autoAccept } : f)),
    }))
    try {
      await api.setFriendAutoAccept(id, autoAccept)
      await get().reloadFriends()
    } catch (e) {
      get().toast('error', String(e))
      await get().reloadFriends()
    }
  },

  respondToOffer: async (id, accept) => {
    // Reflect the choice immediately; the backend then drives the real states.
    const t = get().transfers[id]
    if (t) {
      get().upsertTransfer({
        ...t,
        state: accept ? 'connecting' : 'canceled',
      })
    }
    try {
      await api.respondToOffer(id, accept)
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  toast: (kind, message) => {
    const id = crypto.randomUUID()
    set((s) => ({ toasts: [...s.toasts, { id, kind, message }] }))
    setTimeout(() => get().dismissToast(id), kind === 'error' ? 6000 : 3500)
  },

  dismissToast: (id) =>
    set((s) => ({ toasts: s.toasts.filter((t) => t.id !== id) })),
}))
