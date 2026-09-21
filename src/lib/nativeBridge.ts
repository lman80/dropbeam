import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { api, HAS_TAURI, type Settings, locationsApi, type SharedLocation, type LocationPage } from './api'
import { useStore, rememberLocationUpload, type View } from '../store'
import { MOBILE_UI } from './platform'
import { friendOnlineState } from './presence'
import { transferSharePaths } from './mobilePick'
import { setNativeShellActive } from './nativeShell'
import { changedSnapshots, dispatchNativeCall, type BridgeArgs, type BridgeHandlers } from './nativeBridgeProtocol'
import { nativeReply, nativeChatSource, nativeTransfers, nativeThread } from './nativeChatBridge'
import { withMobileFileSource } from '../components/MobileFileSheet'
import { restoredChatTransfer } from './chatTransfer'
import { nativeBrowserPage, nativeHistoryPaths, locationChild, requireLocationRight } from './nativePhase3'
import { appVersion } from './updater'
import { searchGifs, type GifResult } from './gif'

declare global {
  interface Window { __dbBridge?: { call(id: number, name: string, args: BridgeArgs): Promise<void> } }
}
const st = () => useStore.getState()
const string = (a: BridgeArgs, key: string) => {
  if (typeof a[key] !== 'string') throw new Error(`Missing ${key}`)
  return a[key] as string
}
const paths = (a: BridgeArgs) => {
  if (!Array.isArray(a.paths) || !a.paths.every(p => typeof p === 'string')) throw new Error('Invalid paths')
  return a.paths as string[]
}
const handlers: BridgeHandlers = {
  pickFiles: a => {
    const source = nativeChatSource(a)
    return withMobileFileSource(source, source === 'photos' ? api.pickPhotos : api.pickFiles)
  },
  sendToFriend: async a => { await storeAction(() => st().sendToFriend(string(a, 'friendId'), paths(a))); st().setPendingSend(null) },
  quickSend: async a => { await storeAction(() => st().sendPaths(paths(a))); st().setPendingSend(null) },
  dismissSend: () => st().setPendingSend(null),
  refreshRecipients: async () => {
    await st().refreshMyDevice()
    await Promise.allSettled(st().friends.map(f => st().pingFriend(f.id)))
  },
  receiveWithCode: async a => {
    if (!await st().receiveCode(string(a, 'code'))) throw new Error(st().toasts.at(-1)?.message || 'Could not receive files')
  },
  cancelTransfer: a => api.cancelTransfer(string(a, 'id')),
  retryTransfer: a => {
    const t = st().transfers[string(a, 'id')]
    if (t?.direction === 'receive') {
      if (!t.code) throw new Error('Enter a new receive code to retry this transfer.')
      return handlers.receiveWithCode({ code: t.code })
    }
    return st().retryTransfer(string(a, 'id'))
  },
  openChat: a => st().openChat(string(a, 'friendId')),
  chatThread: async a => {
    const id = string(a, 'friendId')
    if (st().activeChatId !== id) await st().openChat(id)
    await st().loadChats()
    return st().chats[id] ?? []
  },
  closeChat: a => { if (!a.friendId || st().activeChatId === a.friendId) st().closeChat() },
  sendChatText: async a => {
    const id = string(a, 'friendId'), text = string(a, 'text')
    const reply = nativeReply(st().chats[id] ?? [], a.replyTo)
    const files = st().activeChatId === id ? [...st().chatDraftFiles] : []
    if (files.length) {
      st().clearChatDraftFiles()
      await st().shareFilesInChat(id, files, text.trim())
    } else await st().sendChat(id, text, reply)
  },
  sendChatFiles: async a => {
    const id = string(a, 'friendId'), source = nativeChatSource(a)
    if (st().activeChatId !== id) throw new Error('Open the conversation first.')
    const picked = await withMobileFileSource(source, source === 'photos' ? api.pickPhotos : api.pickFiles)
    // A picker can outlive a navigation change. Never attach to the next peer.
    if (st().activeChatId === id) st().stageChatFiles(picked)
  },
  removeChatDraftFile: a => st().unstageChatFile(string(a, 'path')),
  reactToMessage: a => st().reactToMessage(string(a, 'friendId'), string(a, 'messageId'), string(a, 'emoji')),
  editMessage: a => st().editChatMessage(string(a, 'friendId'), string(a, 'messageId'), string(a, 'text')),
  deleteMessage: a => st().deleteChatMessage(string(a, 'friendId'), string(a, 'messageId')),
  markChatRead: async a => {
    const id = string(a, 'friendId')
    if (st().activeChatId === id) return st().markChatRead(id)
    // The store intentionally requires an open thread to clear persisted unread.
    // Suppress navigation for this explicit list action, retaining that policy.
    if (markingRead) return
    markingRead = id
    const previous = st().activeChatId
    try { await st().openChat(id); st().markChatRead(id) }
    finally {
      if (st().activeChatId === id) {
        if (previous) await st().openChat(previous)
        else st().closeChat()
      }
      markingRead = null
    }
  },
  setTyping: a => {
    if (typeof a.bool !== 'boolean') throw new Error('Invalid typing value')
    return api.sendTyping(string(a, 'friendId'), a.bool)
  },
  nativeChatFocus: a => {
    if (typeof a.bool !== 'boolean') throw new Error('Invalid focus value')
    nativeFocused = a.bool
    useStore.setState({ windowFocused: a.bool })
    if (a.bool && st().activeChatId) st().markChatRead(st().activeChatId!)
  },
  retryChatFile: a => {
    const id = string(a, 'friendId'), messageId = string(a, 'messageId')
    const message = st().chats[id]?.find(m => m.id === messageId)
    if (!message?.fileXferId || !message.fromMe) throw new Error('This file cannot be retried here.')
    return st().resendChatFile(id, messageId, message.fileXferId)
  },
  openChatFile: a => api.shareFiles([string(a, 'path')]),
  chatGifs: async a => {
    const key = st().settings?.giphyApiKey.trim()
    if (!key) throw new Error('Add a Giphy key in Settings first.')
    const results = await searchGifs(key, string(a, 'query'))
    if (gifResults.size > 200) gifResults.clear()
    results.forEach(g => gifResults.set(g.id, g))
    return results
  },
  sendChatGif: a => {
    const gif = gifResults.get(string(a, 'id'))
    if (!gif) throw new Error('Search for the GIF again.')
    return st().sendGif(string(a, 'friendId'), { provider: 'giphy', id: gif.id, url: gif.sendUrl, page: gif.pageUrl, w: gif.w, h: gif.h })
  },
  setView: a => {
    const name = string(a, 'name')
    if (!['send', 'friends', 'chat', 'history', 'settings'].includes(name)) throw new Error('Invalid view')
    st().setView(name as View)
  },
  pingFriend: async a => {
    const id = string(a, 'id')
    const online = await st().pingFriend(id)
    const detail = online ? await st().probeFriend(id).catch(() => null) : null
    return { online, path: detail?.path ?? null, rttMs: detail?.rttMs ?? null }
  },
  removeFriend: a => st().removeFriend(string(a, 'id')),
  renameFriend: a => st().renameFriend(string(a, 'id'), string(a, 'name')),
  setAutoAccept: a => {
    if (typeof a.bool !== 'boolean') throw new Error('Invalid auto-accept value')
    return st().setFriendAutoAccept(string(a, 'id'), a.bool)
  },
  myInviteCode: () => api.myInviteCode(),
  addFriendByCode: a => storeAction(() => st().addFriendByCode(string(a, 'code'))),
  acceptFriend: a => storeAction(() => st().acceptFriend(string(a, 'code'))),
  shareFiles: a => api.shareFiles(paths(a)),
  linkDeviceBegin: () => api.linkDeviceBegin(),
  linkDeviceCancel: () => api.linkDeviceCancel(),
  linkDeviceSend: async a => {
    const result = await api.linkDeviceSend(string(a, 'code'))
    await st().reloadFriends()
    await st().refreshMyDevice()
    return { endpointId: result.endpoint_id, name: result.name, deviceKind: result.device_kind }
  },
  updateSettings: a => {
    if (!a.patch || typeof a.patch !== 'object' || Array.isArray(a.patch)) throw new Error('Invalid settings patch')
    return storeAction(() => st().saveSettings(a.patch as Partial<Settings>))
  },
  historyList: async () => { await st().reloadHistory(); return st().history },
  historyClear: async () => { await api.clearHistory(); await st().reloadHistory() },
  historyRemove: async a => { await invoke('remove_history_entry', { id: string(a, 'entryId') }); await st().reloadHistory() },
  historyOpen: a => {
    const entry = st().history.find(e => e.id === string(a, 'entryId'))
    const files = entry ? nativeHistoryPaths(entry) : []
    if (!files.length) throw new Error('No local files are available for this transfer.')
    return api.shareFiles(files)
  },
  recoverableSummaries: () => api.folderHistorySummary(),
  recoverableItems: a => api.listFolderHistory(string(a, 'folder')),
  recoverableRestore: a => { const item = recoveryItem(a); return api.restoreFolderItem(item.folder, item.id) },
  recoverableForget: a => { const item = recoveryItem(a); return api.forgetFolderItem(item.folder, item.id) },
  recoverableEmpty: a => api.clearFolderHistory(string(a, 'folder')),
  recoverableEmptyAll: () => api.clearAllFolderHistory(),
  locationsList: () => locationSnapshot(),
  locationsRefresh: async () => { await refreshLocations(); return locationSnapshot() },
  browserList: async a => nativeBrowserPage(await locationRequest<LocationPage>(a, 'ls', { cursor: a.cursor, query: a.query ?? '', sort: 'name' })),
  browserDownload: a => locationRequest(a, 'download', { paths: names(a).map(n => locationChild(string(a, 'path'), n)) }),
  browserUpload: async a => {
    const friendId = string(a, 'friendId'), locationId = string(a, 'locationId'), path = string(a, 'path')
    checkLocation(a, 'upload')
    const source = string(a, 'source')
    let picked: string[]
    if (source === 'folder') { const folder = await pickNativeFolder(); picked = folder ? [folder] : [] }
    else { const src = nativeChatSource(a); picked = await withMobileFileSource(src, src === 'photos' ? api.pickPhotos : api.pickFiles) }
    if (!picked.length) return null
    checkLocation(a, 'upload')
    const update = await locationsApi.upload(friendId, locationId, path, picked)
    rememberLocationUpload(update.id, friendId, locationId, path, picked)
    if (!st().transfers[update.id]) st().upsertTransfer(update)
    return update
  },
  browserMkdir: a => { checkLocation(a, 'manage'); return locationRequest(a, 'mkdir', { rel_path: locationChild(string(a, 'path'), string(a, 'name').trim()) }) },
  browserRename: a => { checkLocation(a, 'manage'); return locationRequest(a, 'rename', { rel_path: locationChild(string(a, 'path'), string(a, 'from')), to: locationChild(string(a, 'path'), string(a, 'to').trim()) }) },
  browserTrash: async a => {
    checkLocation(a, 'manage')
    const results = []
    for (const name of names(a)) {
      try { const result = await locationRequest<{ trashPath: string }>(a, 'trash', { rel_path: locationChild(string(a, 'path'), name) }); results.push({ name, trashPath: result.trashPath }) }
      catch (e) { results.push({ name, error: String(e) }) }
    }
    return results
  },
  clearTransferCache: () => api.clearTransferCache(),
  exportLogs: async () => { const path = await api.exportDiagnostics(); await api.shareFiles([path]); return path },
  diagnosticsTest: () => api.diagnosticsTest(),
  connectionTest: () => api.irohSelftest(),
  appVersion: () => st().appVer || appVersion(),
  myDeviceInfo: async () => { await st().refreshMyDevice(); return deviceSnapshot() },
  needsName: () => needsName(),
  setDisplayName: async a => {
    const name = string(a, 'name').trim()
    if (!name) throw new Error('Enter your name.')
    await storeAction(() => st().saveSettings({ displayName: name }))
    localStorage.setItem('dropbeam.namedSelf', '1')
    resnapshot?.()
  },
  setAvatar: async a => {
    // Use the existing mobile avatar picker/store action when no path is supplied.
    if (typeof a.path === 'string') {
      const saved = await invoke<Settings>('set_profile_avatar', { path: a.path })
      useStore.setState({ settings: saved })
    } else await withMobileFileSource('photos', async () => { await storeAction(() => st().pickAvatar()); return [] })
  },
  clearAvatar: () => storeAction(() => st().clearAvatar()),
  acceptFolderInvite: async a => {
    const folder = await pickNativeFolder()
    if (!folder) return false
    await api.acceptPair(string(a, 'code'), folder)
    await st().reloadPairs()
    return true
  },
  respondToOffer: a => st().respondToOffer(string(a, 'id'), a.accept === true),
}
// The web Locations view owns its map locally; native uses the same cache and
// locationsApi, without mounting a hidden browser or duplicating engine logic.
let shared: Record<string, SharedLocation[]> = {}
try {
  const cached = JSON.parse(localStorage.getItem('dropbeam.locations') || '{}')
  for (const [id, list] of Object.entries(cached)) if (Array.isArray(list)) shared[id] = list.filter(l => l && typeof l.id === 'string' && typeof l.name === 'string' && typeof l.rights?.upload === 'boolean' && typeof l.rights?.manage === 'boolean')
} catch { /* optional cache */ }
let locationErrors: Record<string, string> = {}
let refreshing: Promise<void> | undefined
let resnapshot: (() => void) | undefined
const needsName = () => !!st().settings && (!st().settings!.displayName.trim() || !localStorage.getItem('dropbeam.namedSelf'))
const deviceSnapshot = () => { const d = st().myDevice; return d ? { name: d.name, endpointId: d.endpoint_id, deviceKind: d.device_kind, accountPub: d.account_pub, linkedDevices: d.linked_devices } : null }
const locationSnapshot = () => st().friends.map(f => ({ friendId: f.id, friendName: f.name, online: friendOnlineState(f.name, st().friendSeen, st().folderStatuses) === true, locations: shared[f.id] ?? [], error: locationErrors[f.id] ?? null }))
function refreshLocations() {
  refreshing ??= (async () => {
    await Promise.allSettled(st().friends.filter(f => f.endpointId).map(async f => {
      try {
        if (friendOnlineState(f.name, st().friendSeen, st().folderStatuses) !== true && !await st().pingFriend(f.id)) return
        shared[f.id] = await locationsApi.list(f.id); delete locationErrors[f.id]
      } catch (e) { locationErrors[f.id] = String(e) }
    }))
    const ids = new Set(st().friends.map(f => f.id))
    shared = Object.fromEntries(Object.entries(shared).filter(([id]) => ids.has(id)))
    try { localStorage.setItem('dropbeam.locations', JSON.stringify(Object.fromEntries(Object.entries(shared).map(([id, list]) => [id, list.map(({ id, name, rights }) => ({ id, name, rights }))])))) } catch { /* optional cache */ }
    resnapshot?.()
  })().finally(() => { refreshing = undefined })
  return refreshing
}
const names = (a: BridgeArgs) => { if (!Array.isArray(a.names) || !a.names.length || !a.names.every(n => typeof n === 'string')) throw new Error('Select files first.'); return a.names as string[] }
function checkLocation(a: BridgeArgs, right: 'read' | 'upload' | 'manage') {
  requireLocationRight(st().friends.some(f => f.id === a.friendId) ? shared[string(a, 'friendId')]?.find(l => l.id === string(a, 'locationId')) : undefined, right)
}
function locationRequest<T>(a: BridgeArgs, kind: string, extra: Record<string, unknown> = {}) {
  checkLocation(a, 'read')
  return locationsApi.request<T>(string(a, 'friendId'), { kind: `locations.${kind}`, id: string(a, 'locationId'), rel_path: string(a, 'path'), ...extra })
}
async function pickNativeFolder(): Promise<string | null> {
  const result = await invoke<{ path: string | null }>('plugin:native-ui|pick_folder')
  return result.path
}
function recoveryItem(a: BridgeArgs) {
  if (!a.item || typeof a.item !== 'object' || Array.isArray(a.item)) throw new Error('Invalid recoverable item')
  const item = a.item as BridgeArgs
  return { folder: string(item, 'folder'), id: string(item, 'id') }
}
async function storeAction(action: () => Promise<void>) {
  const before = st().toasts.at(-1)?.id
  await action()
  const after = st().toasts.at(-1)
  if (after?.kind === 'error' && after.id !== before) throw new Error(after.message)
}
let startup: Promise<void> | undefined
let markingRead: string | null = null
let nativeFocused: boolean | undefined
const gifResults = new Map<string, GifResult>()
let cleanup: (() => void) | undefined
export function startNativeBridge(): Promise<void> {
  if (!MOBILE_UI || !HAS_TAURI) return Promise.resolve()
  startup ??= start()
  return startup
}
async function start() {
  setNativeShellActive(true)
  const stops: UnlistenFn[] = []
  let running = true
  let queue = Promise.resolve()
  const previous = new Map<string, string>()
  const send = (command: 'state' | 'event', payload: Record<string, unknown>) => {
    queue = queue.then(async () => {
      if (running) await invoke(`plugin:native-ui|${command}`, payload)
    }).catch(() => { previous.clear() })
  }
  window.__dbBridge = { call: async (id, name, args) => {
    const reply = await dispatchNativeCall(handlers, id, name, args)
    // A reloaded/destroyed WebView can no longer deliver; Swift times out safely.
    await invoke('plugin:native-ui|reply', reply).catch(() => {})
  } }
  try {
    await invoke('plugin:native-ui|activate')
  } catch {
    delete window.__dbBridge
    setNativeShellActive(false)
    return
  }
  let view = ''
  let lastToast = ''
  let activeChat: string | null = null
  const sync = () => {
    const s = st()
    // WKWebView is deliberately hidden. Its DOM blur/focus cannot describe the
    // native scene; keep the existing receipt/notification gates scene-driven.
    if (nativeFocused !== undefined && s.windowFocused !== nativeFocused) {
      useStore.setState({ windowFocused: nativeFocused })
      return
    }
    const snapshots = {
      friends: s.friends.map(({ secret: _secret, ...friend }) => friend),
      transfers: nativeTransfers(s.order.map(id => s.transfers[id]).filter(Boolean).reverse()
        .filter(t => !(t.state === 'canceled' && !t.fileNames.length)),
        Object.assign({}, ...(s.activeChatId ? s.chats[s.activeChatId] ?? [] : []).map(m => {
          const restored = restoredChatTransfer(m, s.history)
          return restored && m.fileXferId ? { [m.fileXferId]: restored } : {}
        }), s.chatTransfers)).map(t => ({ ...t, sharePaths: t.state === 'completed' ? transferSharePaths(t) : [] })),
      settings: s.settings,
      history: s.history,
      locations: locationSnapshot(),
      needsName: needsName(),
      pendingSend: s.pendingSend ?? [],
      chatOverview: s.chatOverview.map(o => ({ ...o, unread: s.chatUnread[o.peerId] ?? 0 })),
      chatUnread: s.chatUnread,
      chatTyping: s.chatTyping,
      thread: nativeThread(s.activeChatId, s.chats),
      chatDraftFiles: s.chatDraftFiles,
      presence: Object.fromEntries(s.friends.map(f => [f.id, friendOnlineState(f.name, s.friendSeen, s.folderStatuses) === true])),
      myDevice: deviceSnapshot(),
    }
    for (const change of changedSnapshots(previous, snapshots)) send('state', change)
    if (s.activeChatId !== activeChat) {
      activeChat = s.activeChatId
      if (!markingRead || (activeChat && activeChat !== markingRead)) send('event', { name: 'chatOpen', payload: { friendId: activeChat } })
    }
    if (s.view !== view) { view = s.view; send('event', { name: 'view', payload: { name: view } }) }
    const toast = s.toasts.at(-1)
    if (toast && toast.id !== lastToast) {
      lastToast = toast.id
      if (toast.kind === 'error') send('event', { name: 'error', payload: { message: toast.message } })
    }
  }
  resnapshot = sync
  stops.push(useStore.subscribe(sync))
  sync() // Full initial snapshot, even when init is still in progress.
  void st().refreshMyDevice().catch(() => {})
  const timer = window.setInterval(sync, 15_000) // Presence must expire without a store mutation.
  cleanup = () => { resnapshot = undefined; running = false; stops.forEach(stop => stop()); clearInterval(timer) }
  for (const name of ['chat://message', 'friend://presence', 'folder-history://changed', 'folder-invite://incoming', 'locations://changed']) {
    try { stops.push(await listen(name, ({ payload }) => send('event', { name, payload }))) } catch { /* optional event */ }
  }
  try { stops.push(await listen('friends://changed', () => { void st().refreshMyDevice().catch(() => {}) })) } catch { /* store also refreshes */ }
}
if (import.meta.hot) import.meta.hot.dispose(() => {
  cleanup?.()
  delete window.__dbBridge
})
