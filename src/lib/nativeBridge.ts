import { loadLocations, nativeLocationRows, type CheckedLoad } from './locationsLoad'
import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { api, HAS_TAURI, isActive, type Settings, type Friend, type LinkResult, locationsApi, type SharedLocation, type LocationPage } from './api'
import { useStore, rememberLocationUpload, type View } from '../store'
import { MOBILE_UI } from './platform'
import { friendOnlineState } from './presence'
import { transferSharePaths } from './mobilePick'
import { setNativeShellActive } from './nativeShell'
import { changedSnapshots, dispatchNativeCall, deliverNativeReply, pickNativeMedia, nativeAvatarPath, type BridgeArgs, type BridgeHandlers } from './nativeBridgeProtocol'
import { nativeReply, nativeChatSource, nativeTransfers, nativeThread } from './nativeChatBridge'
import { withMobileFileSource } from '../components/MobileFileSheet'
import { restoredChatTransfer } from './chatTransfer'
import { nativeBrowserPage, nativeHistoryPaths, locationChild, requireLocationRight } from './nativePhase3'
import { appVersion } from './updater'
import { searchGifs, type GifResult } from './gif'
import { ownDeviceLabels, personGroups } from './deviceIcons'
import { routeCode } from './codes'
import { nativeFolders, folderLinks } from './nativeFolders'
import { linkedDetail, linkedTitle } from './deviceLink'
import { linkWithCode } from '../components/LinkDeviceModal'

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
    return pickNativeMedia(source, invoke)
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
  // Remove a finished card from the Send list (never an in-flight one).
  dismissTransfer: a => {
    const t = st().transfers[string(a, 'id')]
    if (t && !isActive(t.state)) st().removeTransfer(t.id)
  },
  pauseTransfer: a => api.pauseTransfer(string(a, 'id')),
  verifyTransfer: a => api.verifyTransfer(string(a, 'id')),
  cancelVerify: a => api.cancelVerify(string(a, 'id')),
  // A parked "wait for a direct link" send: go over the relay now.
  forceRelay: a => api.forceRelay(string(a, 'id')),
  // The Send screen's "Have a code?" takes every DropBeam code, like desktop.
  // Folder invites need the native folder picker, so they're handed back to Swift.
  openAnyCode: async a => {
    const route = routeCode(string(a, 'code'))
    switch (route.action) {
      case 'receive': await handlers.receiveWithCode({ code: route.code }); return { kind: 'receive' }
      case 'addFriend': await handlers.addFriendByCode({ code: route.code }); return { kind: 'friend' }
      case 'acceptFriendInvite': await handlers.acceptFriend({ code: route.code }); return { kind: 'friend' }
      case 'acceptFolderInvite': return { kind: 'folderInvite', code: route.code }
      case 'linkDevice': {
        const r = /^dropbeamjoin1:/i.test(route.code) ? await handlers.linkDeviceJoin({ code: route.code }) : await handlers.linkDeviceSend({ code: route.code })
        return { kind: 'linked', name: (r as { name?: string } | undefined)?.name ?? null }
      }
      case 'invalid': throw new Error(route.message)
    }
  },
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
  stageChatFiles: a => {
    const id = string(a, 'friendId')
    // Picking has already replied to Swift. Cancellation never reaches staging.
    if (st().activeChatId !== id) throw new Error('Open the conversation again to attach these files.')
    // O(N) validation/dedup only. No stat/read/preview work on the JS thread;
    // Swift lazily downsamples visible thumbnails after this action replies.
    st().stageChatFiles([...new Set(paths(a))])
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
    if (a.bool) resnapshot?.(true)
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
  linkDeviceSend: async a => linked(await linkWithCode(string(a, 'code'))),
  linkHostBegin: () => api.linkHostBegin(),
  linkHostCancel: () => api.linkHostCancel(),
  linkDeviceJoin: async a => linked(await linkWithCode(string(a, 'code'))),
  accountSyncNow: async () => { await api.accountSyncNow(); await st().refreshMyDevice() },
  accountRemoveDevice: async a => { await api.accountRemoveDevice(string(a, 'endpointId')); await st().reloadFriends() },
  accountLeave: async () => { await api.accountLeave(); await st().reloadFriends() },
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
  locationsList: async () => { await refreshLocations(); return locationSnapshot() },
  // Starts (or queues) a check and answers at once with the rows marked
  // `checking`: each friend's result streams in as its own snapshot push, so a
  // slow or unreachable peer never holds the whole call to a bridge timeout.
  locationsRefresh: () => { void refreshLocations(true).catch(() => {}); return locationSnapshot() },
  browserList: async a => nativeBrowserPage(await locationRequest<LocationPage>(a, 'ls', { cursor: a.cursor, query: a.query ?? '', sort: 'name' })),
  browserDownload: a => locationRequest(a, 'download', { paths: names(a).map(n => locationChild(string(a, 'path'), n)) }),
  browserUpload: async a => {
    const friendId = string(a, 'friendId'), locationId = string(a, 'locationId'), path = string(a, 'path')
    checkLocation(a, 'upload')
    const source = string(a, 'source')
    let picked: string[]
    if (source === 'folder') { const folder = await pickNativeFolder(); picked = folder ? [folder] : [] }
    else { nativeChatSource(a); picked = paths(a) }
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
      const saved = await invoke<Settings>('set_profile_avatar', { path: nativeAvatarPath(a.path) })
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
  // Shared Folders (same engine commands/store actions as desktop FoldersView).
  foldersRefresh: async () => {
    await st().reloadPairs()
    if (!st().myEid) { const eid = await api.myEndpointId().catch(() => null); if (eid) useStore.setState({ myEid: eid }) }
    return folderSnapshot()
  },
  folderSetPaused: async a => {
    if (typeof a.bool !== 'boolean') throw new Error('Invalid pause value')
    await api.setFolderPaused(folderLinks(st().pairs, string(a, 'folderId'))[0].id, a.bool)
    await st().reloadPairs()
  },
  folderStop: async a => { for (const link of folderLinks(st().pairs, string(a, 'folderId'))) await api.stopFolderTransfer(link.id).catch(() => {}) },
  folderVerify: a => api.verifyFolder(folderLinks(st().pairs, string(a, 'folderId'))[0].id),
  folderLeave: async a => {
    for (const link of folderLinks(st().pairs, string(a, 'folderId'))) await storeAction(() => st().removePair(link.id))
  },
  folderRemoveMember: a => storeAction(() => st().removePair(folderMember(a).id)),
  folderSetRole: async a => {
    if (typeof a.bool !== 'boolean') throw new Error('Invalid role value')
    const member = folderMember(a)
    if (!!member.peerIsViewer === a.bool) return
    await api.setMemberRole(member.id, a.bool)
    await st().reloadPairs()
  },
  /** The code of a pending (not yet accepted) invite link, to show again. */
  folderShowInvite: a => api.pairInvite(folderMember(a).id),
  /** A fresh invite code for one more person (desktop "Add person"). */
  folderAddPerson: async a => {
    const code = await api.folderAddPerson(folderLinks(st().pairs, string(a, 'folderId'))[0].id)
    await st().reloadPairs()
    return code
  },
  folderInviteFriend: async a => {
    await api.inviteFriendToFolder(folderLinks(st().pairs, string(a, 'folderId'))[0].id, string(a, 'friendId'))
    await st().reloadPairs()
  },
}
const folderSnapshot = () => { const s = st(); return nativeFolders(s.pairs, s.folderStatuses, s.folderSummaries, s.folderLastSynced, s.myEid, s.friends) }
/** A member link, checked to belong to the named folder (never act on a stale id). */
function folderMember(a: BridgeArgs) {
  const link = folderLinks(st().pairs, string(a, 'folderId')).find(p => p.id === string(a, 'pairId'))
  if (!link) throw new Error('That person is no longer in this folder.')
  return link
}
// The web Locations view owns its map locally; native uses the same cache and
// locationsApi, without mounting a hidden browser or duplicating engine logic.
let shared: Record<string, SharedLocation[]> = {}
try {
  const cached = JSON.parse(localStorage.getItem('dropbeam.locations') || '{}')
  for (const [id, list] of Object.entries(cached)) if (Array.isArray(list)) shared[id] = list.filter(l => l && typeof l.id === 'string' && typeof l.name === 'string' && typeof l.rights?.upload === 'boolean' && typeof l.rights?.manage === 'boolean')
} catch { /* optional cache */ }
let locationResults: Record<string, CheckedLoad> = {}
const locationChecking = new Set<string>()
let refreshing: Promise<void> | undefined
let locationRefreshQueued = false
let resnapshot: ((force?: boolean) => void) | undefined
let pushLocations: (() => void) | undefined
const needsName = () => !!st().settings && (!st().settings!.displayName.trim() || !localStorage.getItem('dropbeam.namedSelf'))
/** A finished link, as Swift shows it (title + detail already worded). */
const linked = async (r: LinkResult) => {
  // Linked into an account: the name comes from the account, so the first-run
  // "What should people call you?" sheet has nothing left to ask.
  try { localStorage.setItem('dropbeam.namedSelf', '1') } catch { /* private mode */ }
  await st().reloadFriends()
  await st().refreshMyDevice()
  resnapshot?.()
  return { endpointId: r.endpoint_id, name: r.name, deviceKind: r.device_kind, deviceOs: r.device_os ?? null,
    friends: r.friends ?? null, messages: r.messages ?? null, title: linkedTitle(r), detail: linkedDetail(r) }
}
const deviceSnapshot = () => {
  const d = st().myDevice
  return d ? { name: d.name, displayName: d.display_name ?? st().settings?.displayName ?? null, endpointId: d.endpoint_id, deviceKind: d.device_kind, deviceOs: d.device_os ?? null, accountPub: d.account_pub, linkedDevices: d.linked_devices,
    devices: (d.devices ?? []).map(x => ({ friendId: x.friend_id, endpointId: x.endpoint_id, name: x.name, deviceKind: x.device_kind, deviceOs: x.device_os, lastSyncMs: x.last_sync_ms, thisDevice: x.this_device })) } : null
}
/** Friends as Swift sees them: own devices flagged and labelled "Your Mac" etc. */
const friendSnapshot = (friends: Friend[], accountPub?: string | null) => {
  const own = friends.filter(f => accountPub && f.accountPub === accountPub)
  const labels = ownDeviceLabels(own)
  const groups = personGroups(friends, accountPub)
  return friends.map(({ secret: _secret, ...friend }) => ({ ...friend, ownDevice: friend.id in labels, ownLabel: labels[friend.id] ?? null, groupedUnder: groups[friend.id] ?? null }))
}
/** Presence per friend; a person reads online when ANY of their devices is. */
const presenceSnapshot = (s: ReturnType<typeof st>) => {
  const out: Record<string, boolean> = Object.fromEntries(s.friends.map(f => [f.id, friendOnlineState(f.name, s.friendSeen, s.folderStatuses) === true]))
  for (const [member, owner] of Object.entries(personGroups(s.friends, s.myDevice?.account_pub))) if (out[member]) out[owner] = true
  return out
}
const presenceOf = (name: string) => friendOnlineState(name, st().friendSeen, st().folderStatuses) === true
const locationSnapshot = () => nativeLocationRows(st().friends, { presence: f => presenceOf(f.name), results: locationResults, shared, checking: locationChecking, now: Date.now() })
// Tauri command errors arrive as plain strings; the engine already words them
// for people ("Couldn’t reach this device…"), and Swift shows them under the
// friend's own heading, so no name prefix.
const plainError = (e: unknown) => (e instanceof Error ? e.message : String(e)).replace(/^Error:\s*/, '') || 'Something went wrong. Try again.'
function refreshLocations(force = false) {
  if (refreshing) { locationRefreshQueued ||= force; return refreshing }
  refreshing = (async () => {
    do {
      locationRefreshQueued = false
      const friends = st().friends.filter(f => f.endpointId)
      friends.forEach(f => locationChecking.add(f.id))
      pushLocations?.()
      await loadLocations({
        friends,
        online: f => presenceOf(f.name),
        probe: id => st().pingFriend(id),
        list: locationsApi.list,
        cached: shared,
        errorText: (_friend, e) => plainError(e),
        onResult: result => {
          // Push each peer as it finishes: one offline peer cannot hide a Linux
          // friend's successful locations behind its timeout.
          shared[result.friendId] = result.locations
          locationResults[result.friendId] = { ...result, at: Date.now() }
          locationChecking.delete(result.friendId)
          pushLocations?.()
        },
      }).finally(() => friends.forEach(f => locationChecking.delete(f.id)))
      const ids = new Set(st().friends.map(f => f.id))
      shared = Object.fromEntries(Object.entries(shared).filter(([id]) => ids.has(id)))
      locationResults = Object.fromEntries(Object.entries(locationResults).filter(([id]) => ids.has(id)))
      try { localStorage.setItem('dropbeam.locations', JSON.stringify(Object.fromEntries(Object.entries(shared).map(([id, list]) => [id, list.map(({ id, name, rights }) => ({ id, name, rights }))])))) } catch { /* optional cache */ }
      pushLocations?.()
    } while (locationRefreshQueued)
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
async function storeAction(action: () => Promise<unknown>) {
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
    await deliverNativeReply(reply, value => invoke('plugin:native-ui|reply', value), () => {
      if (['pickFiles', 'stageChatFiles', 'nativeChatFocus'].includes(name)) resnapshot?.(true)
    }).catch(() => {})
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
  const sync = (force = false) => {
    if (force) previous.clear()
    const s = st()
    // WKWebView is deliberately hidden. Its DOM blur/focus cannot describe the
    // native scene; keep the existing receipt/notification gates scene-driven.
    if (nativeFocused !== undefined && s.windowFocused !== nativeFocused) {
      useStore.setState({ windowFocused: nativeFocused })
      return
    }
    const snapshots = {
      friends: friendSnapshot(s.friends, s.myDevice?.account_pub),
      transfers: nativeTransfers(s.order.map(id => s.transfers[id]).filter(Boolean).reverse()
        .filter(t => !(t.state === 'canceled' && !t.fileNames.length)),
        Object.assign({}, ...(s.activeChatId ? s.chats[s.activeChatId] ?? [] : []).map(m => {
          const restored = restoredChatTransfer(m, s.history)
          return restored && m.fileXferId ? { [m.fileXferId]: restored } : {}
        }), s.chatTransfers)).map(t => ({ ...t, sharePaths: t.direction === 'send' || t.state === 'completed' ? transferSharePaths(t) : [] })),
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
      presence: presenceSnapshot(s),
      myDevice: deviceSnapshot(),
      folders: folderSnapshot(),
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
  pushLocations = () => send('state', { key: 'locations', value: locationSnapshot() })
  resnapshot = sync
  // Store staging is path-only. Coalesce synchronous store notifications into
  // the next turn so serialization cannot hold the picker/staging reply hostage.
  let syncTimer: ReturnType<typeof setTimeout> | undefined
  stops.push(useStore.subscribe(() => {
    syncTimer ??= setTimeout(() => { syncTimer = undefined; if (running) sync() }, 0)
  }))
  sync() // Full initial snapshot, even when init is still in progress.
  void st().refreshMyDevice().catch(() => {})
  const timer = window.setInterval(sync, 15_000) // Presence must expire without a store mutation.
  cleanup = () => { resnapshot = undefined; pushLocations = undefined; running = false; stops.forEach(stop => stop()); clearInterval(timer); clearTimeout(syncTimer) }
  for (const name of ['chat://message', 'friend://presence', 'folder-history://changed', 'folder-invite://incoming', 'locations://changed']) {
    try { stops.push(await listen(name, ({ payload }) => {
      send('event', { name, payload })
      if (name === 'locations://changed') void refreshLocations(true)
    })) } catch { /* optional event */ }
  }
  for (const name of ['friends://changed', 'account://synced', 'link://linked']) {
    try { stops.push(await listen(name, () => { void st().refreshMyDevice().catch(() => {}) })) } catch { /* store also refreshes */ }
  }
  for (const name of ['link://linked', 'account://left', 'link://failed', 'link://progress']) {
    try { stops.push(await listen(name, ({ payload }) => send('event', { name, payload }))) } catch { /* optional event */ }
  }
}
if (import.meta.hot) import.meta.hot.dispose(() => {
  cleanup?.()
  delete window.__dbBridge
})
