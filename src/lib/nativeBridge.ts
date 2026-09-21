import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { api, HAS_TAURI, type Settings } from './api'
import { useStore, type View } from '../store'
import { MOBILE_UI } from './platform'
import { friendOnlineState } from './presence'
import { pickAndSend, transferSharePaths } from './mobilePick'
import { setNativeShellActive } from './nativeShell'
import { changedSnapshots, dispatchNativeCall, type BridgeArgs, type BridgeHandlers } from './nativeBridgeProtocol'

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
  pickAndSend: a => {
    const source = string(a, 'source')
    if (source !== 'photos' && source !== 'files') throw new Error('Invalid source')
    return pickAndSend(source, typeof a.friendId === 'string' ? a.friendId : undefined)
  },
  sendToFriend: a => st().sendToFriend(string(a, 'friendId'), paths(a)),
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
  sendChatText: a => st().sendChat(string(a, 'friendId'), string(a, 'text')),
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
  addFriendByCode: a => st().addFriendByCode(string(a, 'code')),
  acceptFriend: a => st().acceptFriend(string(a, 'code')),
  shareFiles: a => api.shareFiles(paths(a)),
  linkDeviceBegin: () => api.linkDeviceBegin(),
  linkDeviceCancel: () => api.linkDeviceCancel(),
  linkDeviceSend: async a => {
    const result = await api.linkDeviceSend(string(a, 'code'))
    await st().reloadFriends()
    return { endpointId: result.endpoint_id, name: result.name, deviceKind: result.device_kind }
  },
  updateSettings: a => {
    if (!a.patch || typeof a.patch !== 'object' || Array.isArray(a.patch)) throw new Error('Invalid settings patch')
    return st().saveSettings(a.patch as Partial<Settings>)
  },
  respondToOffer: a => st().respondToOffer(string(a, 'id'), a.accept === true),
}
let startup: Promise<void> | undefined
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
  let overlay = false
  let view = ''
  let lastToast = ''
  const sync = () => {
    const s = st()
    const d = s.myDevice
    const snapshots = {
      friends: s.friends.map(({ secret: _secret, ...friend }) => friend),
      transfers: s.order.map(id => s.transfers[id]).filter(Boolean).reverse()
        .filter(t => !(t.state === 'canceled' && !t.fileNames.length))
        .map(t => ({ ...t, sharePaths: t.state === 'completed' ? transferSharePaths(t) : [] })),
      settings: s.settings,
      chatOverview: s.chatOverview.map(o => ({ ...o, unread: s.chatUnread[o.peerId] ?? 0 })),
      presence: Object.fromEntries(s.friends.map(f => [f.id, friendOnlineState(f.name, s.friendSeen, s.folderStatuses) === true])),
      myDevice: d ? { name: d.name, endpointId: d.endpoint_id, deviceKind: d.device_kind, accountPub: d.account_pub, linkedDevices: d.linked_devices } : null,
    }
    for (const change of changedSnapshots(previous, snapshots)) send('state', change)
    const show = !!s.pendingSend?.length
    if (show !== overlay) { overlay = show; send('event', { name: 'webOverlay', payload: { visible: show } }) }
    if (s.view !== view) { view = s.view; send('event', { name: 'view', payload: { name: view } }) }
    const toast = s.toasts.at(-1)
    if (toast && toast.id !== lastToast) {
      lastToast = toast.id
      if (toast.kind === 'error') send('event', { name: 'error', payload: { message: toast.message } })
    }
  }
  stops.push(useStore.subscribe(sync))
  sync() // Full initial snapshot, even when init is still in progress.
  void st().refreshMyDevice().catch(() => {})
  const timer = window.setInterval(sync, 15_000) // Presence must expire without a store mutation.
  cleanup = () => { running = false; stops.forEach(stop => stop()); clearInterval(timer) }
  for (const name of ['chat://message', 'friend://presence']) {
    try { stops.push(await listen(name, ({ payload }) => send('event', { name, payload }))) } catch { /* optional event */ }
  }
  try { stops.push(await listen('friends://changed', () => { void st().refreshMyDevice().catch(() => {}) })) } catch { /* store also refreshes */ }
}
if (import.meta.hot) import.meta.hot.dispose(() => {
  cleanup?.()
  delete window.__dbBridge
})
