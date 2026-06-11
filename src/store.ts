import { create } from 'zustand'
import {
  api,
  onChatMessage,
  onFolderStatus,
  onFolderSynced,
  onFriendsChanged,
  onHistoryChanged,
  onOpenFileSend,
  onPairsChanged,
  onTransferUpdate,
  type ChatMessage,
  type ChatOverview,
  type FolderStatus,
  type Friend,
  type HistoryEntry,
  type Pair,
  type PairUpdate,
  type Settings,
  type TransferUpdate,
} from './lib/api'
import { setSpeedUnit } from './lib/format'
import { appVersion, checkUpdate, installUpdate as runInstall } from './lib/updater'

/** Used only if `get_settings` fails at startup, so the app still renders. */
const DEFAULT_SETTINGS: Settings = {
  downloadDir: '',
  displayName: '',
  theme: 'system',
  minimizeToTray: true,
  launchAtLogin: false,
  preferDirectP2p: true,
  customRelay: '',
  customRelayPass: '',
  notifyOnComplete: true,
  playSounds: true,
  directMode: true,
  uploadLimitMbps: 0,
  showMegabits: false,
  requireDirect: false,
  avatar: '',
  notifyOnMessage: true,
}

/** Resolve `p`, but never hang: fall back on error OR after `ms`, logging why. */
async function guarded<T>(p: Promise<T>, fallback: T, label: string, ms = 9000): Promise<T> {
  try {
    return await Promise.race([
      p,
      new Promise<T>((_, rej) => setTimeout(() => rej(new Error(`timed out after ${ms}ms`)), ms)),
    ])
  } catch (e) {
    api.frontendLog(`init: ${label} failed: ${String(e)}`).catch(() => {})
    return fallback
  }
}
import { playError, playIncoming, playOffer, playReceived, playSent } from './lib/sounds'

/** Leading-edge throttle (per pair+direction) so a burst of synced files cues once. */
const folderSoundThrottle = new Map<string, number>()

/** When each transfer started moving bytes, to compute a final average speed. */
const transferStart = new Map<string, number>()

interface UpdateState {
  version: string
  notes: string
  installing: boolean
  progress: number
}

export type View = 'send' | 'friends' | 'folders' | 'chat' | 'history' | 'settings'

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
  /** Final stats per completed transfer: how long it took + average speed. */
  transferSummaries: Record<string, { durationMs: number; avgBps: number }>
  /** macOS: warning if the app is installed/running in a way that breaks folder
   * permissions every launch (null = fine / non-macOS). */
  installHint: string | null
  dragHovering: boolean
  history: HistoryEntry[]
  pairs: Pair[]
  friends: Friend[]
  folderStatuses: Record<string, FolderStatus>
  /** When each folder pair last synced a file (ms) — for the "synced 2m ago" label. */
  folderLastSynced: Record<string, number>
  /** Files picked/dropped that are awaiting a "send to whom?" choice. */
  pendingSend: string[] | null
  /** Last time (ms) a friend was seen online, keyed by lowercased name. */
  friendSeen: Record<string, number>
  toasts: Toast[]
  defaultDownloadDir: string
  appVer: string
  update: UpdateState | null
  checkingUpdate: boolean
  /** Set when the last update check couldn't reach the server (offer manual DL). */
  updateError: string | null

  init: () => Promise<void>
  checkForUpdates: (manual: boolean) => Promise<void>
  installUpdate: () => Promise<void>
  setView: (v: View) => void
  setDragHovering: (v: boolean) => void
  setPendingSend: (paths: string[] | null) => void
  markFriendSeen: (name: string) => void
  applyTheme: (theme: Settings['theme']) => void
  saveSettings: (patch: Partial<Settings>) => Promise<void>
  pickAvatar: () => Promise<void>
  clearAvatar: () => Promise<void>
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
  addFriendByCode: (code: string) => Promise<void>
  renameFriend: (id: string, name: string) => Promise<void>
  removeFriend: (id: string) => Promise<void>
  setFriendAutoAccept: (id: string, autoAccept: boolean) => Promise<void>
  respondToOffer: (id: string, accept: boolean) => Promise<void>
  pingFriend: (id: string) => Promise<boolean>
  // Chat (experimental) — direct messages with friends.
  chats: Record<string, ChatMessage[]>
  chatOverview: ChatOverview[]
  chatUnread: Record<string, number>
  activeChatId: string | null
  loadChats: () => Promise<void>
  openChat: (friendId: string) => Promise<void>
  closeChat: () => void
  sendChat: (friendId: string, text: string) => Promise<void>
  shareFilesInChat: (friendId: string, paths: string[]) => Promise<void>
  addChatMessage: (m: ChatMessage) => void
  toast: (kind: Toast['kind'], message: string) => void
  dismissToast: (id: string) => void
}

// Guard against Tauri's occasional double-fire of a single OS file drop.
let lastDropSig = ''
let lastDropAt = 0
// Same guard for friend sends: a double-fired drop onto a friend (the chooser or
// the menu-bar popover drag-to-send) would otherwise send the same files twice.
let lastFriendSig = ''
let lastFriendAt = 0
// In-flight receives by ticket, so pasting the same code twice can't start two
// pulls of the same files racing each other into "name (1)" duplicates.
const activeReceives = new Map<string, string>()

export const useStore = create<AppStore>((set, get) => ({
  ready: false,
  view: 'send',
  settings: null,
  transfers: {},
  order: [],
  transferSummaries: {},
  installHint: null,
  dragHovering: false,
  history: [],
  pairs: [],
  friends: [],
  folderStatuses: {},
  folderLastSynced: {},
  pendingSend: null,
  friendSeen: {},
  chats: {},
  chatOverview: [],
  chatUnread: {},
  activeChatId: null,
  toasts: [],
  defaultDownloadDir: '',
  appVer: '',
  update: null,
  checkingUpdate: false,
  updateError: null,

  init: async () => {
    // Bulletproof startup: every call is guarded with a fallback + timeout and
    // we ALWAYS reach ready:true, so a slow or failing backend renders the app
    // (with defaults) rather than hanging on the loading screen forever. Any
    // failure is logged to the app log file (frontend_log) for diagnosis.
    const [settings, history, pairs, friends, statuses, defaultDownloadDir] = await Promise.all([
      guarded(api.getSettings(), DEFAULT_SETTINGS, 'getSettings'),
      guarded(api.getHistory(), [], 'getHistory'),
      guarded(api.listPairs(), [], 'listPairs'),
      guarded(api.listFriends(), [], 'listFriends'),
      guarded(api.getFolderStatuses(), [], 'getFolderStatuses'),
      guarded(api.getDefaultDownloadDir(), '', 'getDefaultDownloadDir'),
    ])
    try {
      get().applyTheme(settings.theme)
    } catch {
      /* theme is non-critical */
    }
    setSpeedUnit(settings.showMegabits)
    const folderStatuses: Record<string, FolderStatus> = {}
    statuses.forEach((s) => (folderStatuses[s.pairId] = s))
    set({ settings, history, pairs, friends, folderStatuses, defaultDownloadDir, ready: true })

    // Only the main window owns window-scoped side effects (sounds, update check).
    // The popover/HUD webviews share this store but must not double/triple-fire.
    const isOverlay =
      typeof document !== 'undefined' &&
      document.documentElement.classList.contains('overlay-window')

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
    // A shared-folder file just left/arrived. Play an opt-in, per-folder cue so
    // you can *hear* a folder syncing. Off by default (folders sync constantly);
    // the toggle lives on each folder card and is stored in localStorage.
    onFolderSynced((s) => {
      // Remember when this folder last moved a file, for the "Up to date · 2m ago"
      // resting status (a Dropbox-style reassurance the folder card lacked).
      set((st) => ({ folderLastSynced: { ...st.folderLastSynced, [s.pairId]: Date.now() } }))
      if (isOverlay) return
      let on = false
      try {
        on = localStorage.getItem(`folder-sound-${s.pairId}`) === 'on'
      } catch {
        /* localStorage unavailable — treat as off */
      }
      if (!on) return
      const key = `${s.pairId}:${s.direction}`
      const now = Date.now()
      if (now - (folderSoundThrottle.get(key) ?? 0) < 1500) return
      folderSoundThrottle.set(key, now)
      if (s.direction === 'send') playSent()
      else playReceived()
    })
    // Chat lives in the main window only. Load the conversation previews and
    // listen for live messages (from friends, and our own echoed sends).
    if (!isOverlay) {
      get().loadChats()
      // macOS: warn if we're running translocated / from Downloads (folder perms
      // won't stick). Null on a proper install or other platforms.
      api
        .macosInstallHint()
        .then((h) => {
          if (h) set({ installHint: h })
        })
        .catch(() => {})
      onChatMessage((m) => {
        get().addChatMessage(m)
        if (
          !m.fromMe &&
          get().activeChatId !== m.peerId &&
          (get().settings?.playSounds ?? true)
        ) {
          playIncoming()
        }
      })
    }
    // The control channel can learn the peer's name after the fact — reload pairs
    // so the folder shows who's in it (and clears the stale "waiting" state).
    onPairsChanged(() => get().reloadPairs())
    onFriendsChanged(() => get().reloadFriends())

    // "Send with DropBeam" (Windows right-click): open the send chooser for the
    // file the app was launched with (cold start) or that a second launch
    // forwarded (the native side already brought the window to front).
    api
      .takeLaunchFile()
      .then((f) => {
        if (f) get().setPendingSend([f])
      })
      .catch(() => {})
    onOpenFileSend((path) => {
      if (path) get().setPendingSend([path])
    })

    appVersion().then((v) => set({ appVer: v }))
    // Only the main window owns the update check (the popover/HUD share state but
    // shouldn't each trigger their own launch check).
    if (!isOverlay) get().checkForUpdates(false)
  },

  checkForUpdates: async (manual) => {
    if (get().checkingUpdate) return
    set({ checkingUpdate: true, updateError: null })
    // Retry a few times: GitHub (where releases live) is often only INTERMITTENTLY
    // reachable — e.g. throttled from China — so a first failure may succeed on a
    // retry. On give-up we record the error so Settings can offer a manual download.
    let lastErr: unknown = null
    try {
      for (let attempt = 0; attempt < 3; attempt++) {
        try {
          const info = await checkUpdate()
          if (info) {
            set({ update: { version: info.version, notes: info.notes, installing: false, progress: 0 } })
            if (manual) get().toast('info', `Update available: v${info.version}`)
          } else if (manual) {
            get().toast('success', "You're on the latest version")
          }
          return
        } catch (e) {
          lastErr = e
          if (attempt < 2) await new Promise((r) => setTimeout(r, 1500))
        }
      }
      set({ updateError: String(lastErr) })
      if (manual) get().toast('error', "Couldn't reach the update server — download manually below")
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

  setPendingSend: (pendingSend) => set({ view: 'send', pendingSend }),

  markFriendSeen: (name) => {
    const key = name.trim().toLowerCase()
    if (key) set((s) => ({ friendSeen: { ...s.friendSeen, [key]: Date.now() } }))
  },

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
      // iroh-only Quick Send: stage over the direct P2P engine; the receiver
      // pulls with the Direct link/QR.
      const u = await api.irohSend(paths)
      get().upsertTransfer(u)
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  receiveCode: async (code) => {
    code = code.trim()
    if (!code) return
    try {
      // iroh-only: receives use the Direct ticket from the sender's link/QR.
      if (!code.startsWith('direct')) {
        get().toast(
          'error',
          'Paste the Direct link or scan the QR to receive. (Short word-codes return once a rendezvous broker is set up — see SHORT-CODES.md.)',
        )
        return
      }
      // One receive per ticket at a time: a double-paste/double-click would
      // start two pulls of the same files racing each other into duplicates.
      const priorId = activeReceives.get(code)
      if (priorId) {
        const prior = get().transfers[priorId]
        if (prior && !['completed', 'failed', 'canceled'].includes(prior.state)) {
          get().toast('info', 'Already receiving this transfer.')
          return
        }
        activeReceives.delete(code)
      }
      const u = await api.irohReceive(code)
      activeReceives.set(code, u.id)
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
    if (patch.showMegabits !== undefined) setSpeedUnit(patch.showMegabits)
    try {
      const saved = await api.updateSettings(next)
      set({ settings: saved })
    } catch (e) {
      get().toast('error', `Couldn't save settings: ${e}`)
    }
  },

  pickAvatar: async () => {
    try {
      const saved = await api.setProfileAvatar()
      set({ settings: saved })
    } catch (e) {
      get().toast('error', `Couldn't set picture: ${e}`)
    }
  },
  clearAvatar: async () => {
    try {
      const saved = await api.clearProfileAvatar()
      set({ settings: saved })
    } catch (e) {
      get().toast('error', `Couldn't remove picture: ${e}`)
    }
  },

  upsertTransfer: (u) => {
    const prev = get().transfers[u.id]
    // A friend transfer that actually connected means they were online just now.
    if (
      u.friendName &&
      (u.state === 'connecting' || u.state === 'transferring' || u.state === 'completed')
    ) {
      const key = u.friendName.trim().toLowerCase()
      const last = get().friendSeen[key] ?? 0
      if (Date.now() - last > 5000) {
        set((s) => ({ friendSeen: { ...s.friendSeen, [key]: Date.now() } }))
      }
    }
    // Sounds fire on meaningful state changes only (not on every progress tick).
    if ((get().settings?.playSounds ?? true) && (!prev || prev.state !== u.state)) {
      if (u.state === 'completed') {
        if (u.direction === 'send') playSent()
        else playReceived()
      } else if (u.state === 'failed' || u.state === 'canceled') {
        // A transfer errored out or was canceled — a soft descending "uh-oh".
        playError()
      } else if (u.state === 'waitingForAccept') {
        playOffer()
      } else if (u.direction === 'receive' && !prev) {
        // First time we see an incoming transfer (auto-accept) — a soft cue.
        // (failed/canceled are handled above, so this is a live arrival.)
        playIncoming()
      }
    }
    // Time the transfer so we can show a final summary (duration + avg speed).
    if (u.state === 'transferring' && (!prev || prev.state !== 'transferring')) {
      transferStart.set(u.id, Date.now()) // bytes just started moving
    } else if (!transferStart.has(u.id)) {
      transferStart.set(u.id, Date.now()) // fallback: first time we saw it
    }
    if (u.state === 'completed' && u.bytesTotal > 0) {
      const start = transferStart.get(u.id)
      if (start) {
        const durationMs = Math.max(1, Date.now() - start)
        const avgBps = u.bytesTotal / (durationMs / 1000)
        set((s) => ({
          transferSummaries: { ...s.transferSummaries, [u.id]: { durationMs, avgBps } },
        }))
      }
    }
    if (u.state === 'completed' || u.state === 'failed' || u.state === 'canceled') {
      transferStart.delete(u.id)
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
    // De-dupe a double-fired send of the same files to the same friend within
    // ~1.5s (a doubled OS drop event would otherwise transfer everything twice).
    const sig = `${id}|${paths.join('|')}`
    const now = Date.now()
    if (sig === lastFriendSig && now - lastFriendAt < 1500) return
    lastFriendSig = sig
    lastFriendAt = now
    set({ view: 'send' })
    try {
      const u = await api.sendToFriend(id, paths)
      get().upsertTransfer(u)
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  // --- Chat (experimental) ---------------------------------------------------
  loadChats: async () => {
    const chatOverview = await api.listChats().catch(() => [])
    set({ chatOverview })
  },

  openChat: async (friendId) => {
    set({ activeChatId: friendId, view: 'chat' })
    const msgs = await api.getChatMessages(friendId).catch(() => [])
    set((s) => ({
      chats: { ...s.chats, [friendId]: msgs },
      chatUnread: { ...s.chatUnread, [friendId]: 0 },
    }))
  },

  closeChat: () => set({ activeChatId: null }),

  sendChat: async (friendId, text) => {
    const body = text.trim()
    if (!body) return
    // The Rust side persists the message and IMMEDIATELY emits it back over
    // `chat://message` (status 'sending'), then a background outbox auto-retries
    // and emits 'sent'/'failed' — so the bubble paints + recovers on its own. We
    // just trigger the send and let those echoes drive the thread (adding it
    // optimistically here would double-render every message against that echo).
    try {
      const m = await api.sendChatMessage(friendId, body)
      get().addChatMessage(m)
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  shareFilesInChat: async (friendId, paths) => {
    paths = paths.filter(Boolean)
    if (!paths.length) return
    try {
      const t = await api.sendToFriend(friendId, paths)
      get().upsertTransfer(t)
      const names = paths.map((p) => p.split(/[/\\]/).pop() || p)
      const m = await api.sendChatFileNote(friendId, names, t.bytesTotal || 0, paths)
      get().addChatMessage(m)
    } catch (e) {
      get().toast('error', String(e))
    }
  },

  addChatMessage: (m) => {
    // An incoming message means that friend is reachable right now.
    if (!m.fromMe) {
      const f = get().friends.find((fr) => fr.id === m.peerId)
      if (f) get().markFriendSeen(f.name)
    }
    set((s) => {
      const thread = s.chats[m.peerId] ?? []
      const idx = thread.findIndex((x) => x.id === m.id)
      // A repeat id is an UPDATE (e.g. a delivery-status change), not a new
      // message — replace it in place and don't re-bump unread/count.
      const isNew = idx < 0
      const nextThread = isNew
        ? [...thread, m].sort((a, b) => a.ts - b.ts)
        : thread.map((x) => (x.id === m.id ? m : x))
      const prev = s.chatOverview.find((o) => o.peerId === m.peerId)
      const last = nextThread[nextThread.length - 1]
      const lastText =
        last.kind === 'file'
          ? last.files.length === 1
            ? `📎 ${last.files[0]}`
            : `📎 ${last.files.length} files`
          : last.text
      const row: ChatOverview = {
        peerId: m.peerId,
        lastText,
        lastTs: last.ts,
        lastFromMe: last.fromMe,
        count: isNew ? (prev ? prev.count + 1 : nextThread.length) : (prev?.count ?? nextThread.length),
      }
      const chatOverview = [row, ...s.chatOverview.filter((o) => o.peerId !== m.peerId)].sort(
        (a, b) => b.lastTs - a.lastTs,
      )
      const chatUnread =
        isNew && !m.fromMe && s.activeChatId !== m.peerId
          ? { ...s.chatUnread, [m.peerId]: (s.chatUnread[m.peerId] ?? 0) + 1 }
          : s.chatUnread
      return { chats: { ...s.chats, [m.peerId]: nextThread }, chatOverview, chatUnread }
    })
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

  addFriendByCode: async (code) => {
    const friend = await api.addFriendByCode(code)
    await get().reloadFriends()
    get().toast('success', `Added ${friend.name}`)
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

  pingFriend: async (id) => {
    try {
      const online = await api.pingFriend(id)
      if (online) {
        const f = get().friends.find((x) => x.id === id)
        if (f) get().markFriendSeen(f.name)
      }
      return online
    } catch {
      return false
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
