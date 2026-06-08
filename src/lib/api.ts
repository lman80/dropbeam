// Typed bridge to the Rust backend: command wrappers + event subscriptions.
// Types mirror the serde structs in src-tauri/src/models.rs (camelCase).

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { mockApi, mockListen } from './mock'

/** True when running inside the real Tauri app (vs. a plain browser preview). */
export const HAS_TAURI =
  typeof window !== 'undefined' && '__TAURI_INTERNALS__' in window

export type Direction = 'send' | 'receive'
export type TransferState =
  | 'starting'
  | 'waitingForPeer'
  | 'connecting'
  | 'waitingForAccept'
  | 'transferring'
  | 'completed'
  | 'failed'
  | 'canceled'
export type Locality = 'unknown' | 'local' | 'internet'

export interface TransferUpdate {
  id: string
  direction: Direction
  state: TransferState
  code: string | null
  fileNames: string[]
  fileCount: number
  percent: number
  bytesDone: number
  bytesTotal: number
  speedBps: number
  etaSeconds: number | null
  locality: Locality
  peer: string | null
  error: string | null
  outDir: string | null
  /** Set when sending straight to a friend — shows "Sending to {name}", no code. */
  friendName: string | null
}

export interface HistoryEntry {
  id: string
  direction: Direction
  fileNames: string[]
  bytesTotal: number
  peer: string | null
  locality: Locality
  code: string | null
  state: TransferState
  timestampMs: number
  error: string | null
  outDir: string | null
}

export interface Settings {
  downloadDir: string
  displayName: string
  theme: 'system' | 'light' | 'dark'
  minimizeToTray: boolean
  launchAtLogin: boolean
  preferDirectP2p: boolean
  customRelay: string
  customRelayPass: string
  notifyOnComplete: boolean
  playSounds: boolean
  /** Opt in to the new direct P2P engine (iroh) for Quick Send. */
  directMode: boolean
}

export type PairRole = 'a' | 'b'
export type DeleteMode = 'trash' | 'permanent'

export interface Pair {
  id: string
  role: PairRole
  peerName: string
  secret: string
  folder: string
  twoWay: boolean
  mirror: boolean
  autoDelete: boolean
  deleteMode: DeleteMode
  createdAt: number
  /** The folder peer's iroh device key, for direct sync. Null until exchanged. */
  endpointId: string | null
}

/** A restorable deleted/overwritten file in a mirror folder's history. */
export interface HistoryItem {
  id: string
  relPath: string
  size: number
  reason: string
  timestampMs: number
}

/** A named peer you can send to directly — no code, no QR. */
export interface Friend {
  id: string
  role: PairRole
  name: string
  secret: string
  createdAt: number
  autoAccept: boolean
  /** The friend's iroh device id, learned at pairing — enables direct sends. */
  endpointId: string | null
}

/** One message in a chat with a friend (experimental). */
export interface ChatMessage {
  id: string
  /** The friend id this conversation belongs to. */
  peerId: string
  fromMe: boolean
  /** "text" or "file". */
  kind: 'text' | 'file'
  text: string
  /** For file messages: the names of the files shared. */
  files: string[]
  /** For file messages: total bytes. */
  bytes: number
  /** For file messages: the local path to the (first) file on THIS device —
   * sender's source or receiver's saved copy. Enables preview + open. */
  path: string | null
  /** Delivery state for messages WE sent: 'sending' | 'sent' | 'failed'. Null for
   * received messages. */
  status: 'sending' | 'sent' | 'failed' | null
  ts: number
}

/** A preview of one conversation, for the chat list. */
export interface ChatOverview {
  peerId: string
  lastText: string
  lastTs: number
  lastFromMe: boolean
  count: number
}

export type FolderState = 'idle' | 'sending' | 'receiving' | 'waiting' | 'error'

export interface FolderStatus {
  pairId: string
  state: FolderState
  queued: number
  sendingFile: string | null
  percent: number
  bytesDone: number
  bytesTotal: number
  speedBps: number
  etaSeconds: number | null
  detail: string | null
  peerOnline: boolean
  peerName: string | null
  locality: Locality
}

export interface PairUpdate {
  id: string
  twoWay?: boolean
  mirror?: boolean
  autoDelete?: boolean
  deleteMode?: DeleteMode
  peerName?: string
}

const realApi = {
  sendFiles: (paths: string[]) => invoke<TransferUpdate>('send_files', { paths }),
  receiveFiles: (code: string) => invoke<TransferUpdate>('receive_files', { code }),
  // Direct engine (iroh) Quick Send — same UI, P2P transport.
  irohSend: (paths: string[]) => invoke<TransferUpdate>('iroh_send', { paths }),
  irohReceive: (ticket: string) => invoke<TransferUpdate>('iroh_receive', { ticket }),
  irohSelftest: () => invoke<string>('iroh_selftest'),
  cancelTransfer: (id: string) => invoke<void>('cancel_transfer', { id }),
  /** Write a line into the native app log file (for diagnosing remote issues). */
  frontendLog: (msg: string) => invoke<void>('frontend_log', { msg }),
  /** A file the app was launched to send (Windows "Send with DropBeam"). */
  takeLaunchFile: () => invoke<string | null>('take_launch_file'),
  getSettings: () => invoke<Settings>('get_settings'),
  updateSettings: (settings: Settings) => invoke<Settings>('update_settings', { settings }),
  getHistory: () => invoke<HistoryEntry[]>('get_history'),
  clearHistory: () => invoke<void>('clear_history'),
  pickFiles: () => invoke<string[]>('pick_files'),
  pickDirectory: () => invoke<string | null>('pick_directory'),
  revealPath: (path: string) => invoke<void>('reveal_path', { path }),
  openPath: (path: string) => invoke<void>('open_path', { path }),
  getDefaultDownloadDir: () => invoke<string>('get_default_download_dir'),
  // Shared Drop Folders. Tauri v2 maps camelCase JS keys → snake_case Rust params,
  // so the keys here MUST be camelCase (e.g. twoWay, not two_way).
  createPair: (folder: string, twoWay: boolean, peerName?: string, mirror?: boolean) =>
    invoke<{ pair: Pair; invite: string }>('create_pair', { folder, twoWay, peerName, mirror }),
  acceptPair: (invite: string, folder: string) =>
    invoke<Pair>('accept_pair', { invite, folder }),
  listPairs: () => invoke<Pair[]>('list_pairs'),
  updatePair: (u: PairUpdate) =>
    invoke<Pair>('update_pair', {
      id: u.id,
      twoWay: u.twoWay,
      mirror: u.mirror,
      autoDelete: u.autoDelete,
      deleteMode: u.deleteMode,
      peerName: u.peerName,
    }),
  removePair: (id: string) => invoke<void>('remove_pair', { id }),
  pairInvite: (id: string) => invoke<string>('pair_invite', { id }),
  getFolderStatuses: () => invoke<FolderStatus[]>('get_folder_statuses'),
  listFolderHistory: (pairId: string) =>
    invoke<HistoryItem[]>('list_folder_history', { pairId }),
  restoreFolderItem: (pairId: string, itemId: string) =>
    invoke<void>('restore_folder_item', { pairId, itemId }),
  forgetFolderItem: (pairId: string, itemId: string) =>
    invoke<void>('forget_folder_item', { pairId, itemId }),
  // Friends — named peers you send to directly.
  createFriend: (friendName: string) =>
    invoke<{ friend: Friend; invite: string }>('create_friend', { friendName }),
  acceptFriend: (invite: string) => invoke<Friend>('accept_friend', { invite }),
  listFriends: () => invoke<Friend[]>('list_friends'),
  renameFriend: (id: string, name: string) =>
    invoke<void>('rename_friend', { id, name }),
  removeFriend: (id: string) => invoke<void>('remove_friend', { id }),
  pingFriend: (id: string) => invoke<boolean>('ping_friend', { id }),
  setFriendAutoAccept: (id: string, autoAccept: boolean) =>
    invoke<void>('set_friend_auto_accept', { id, autoAccept }),
  respondToOffer: (id: string, accept: boolean) =>
    invoke<void>('respond_to_offer', { id, accept }),
  friendInvite: (id: string) => invoke<string>('friend_invite', { id }),
  sendToFriend: (id: string, paths: string[]) =>
    invoke<TransferUpdate>('send_to_friend', { id, paths }),
  /** Your permanent, reusable DropBeam code (stable device key + name). */
  myInviteCode: () => invoke<string>('my_invite_code'),
  /** Add a friend from their permanent code; auto-fills their name, two-way. */
  addFriendByCode: (code: string) => invoke<Friend>('add_friend_by_code', { code }),
  /** macOS: a warning if the app is running from a spot that breaks folder
   * permissions every launch (translocation / Downloads / DMG). Null when fine. */
  macosInstallHint: () => invoke<string | null>('macos_install_hint'),
  // Chat (experimental) — direct messages + file shares with friends over iroh.
  getChatMessages: (friendId: string) =>
    invoke<ChatMessage[]>('get_chat_messages', { friendId }),
  listChats: () => invoke<ChatOverview[]>('list_chats'),
  sendChatMessage: (friendId: string, text: string) =>
    invoke<ChatMessage>('send_chat_message', { friendId, text }),
  sendChatFileNote: (friendId: string, names: string[], bytes: number, paths: string[]) =>
    invoke<ChatMessage>('send_chat_file_note', { friendId, names, bytes, paths }),
}

export const api: typeof realApi = HAS_TAURI ? realApi : (mockApi as typeof realApi)

export function onFolderStatus(cb: (s: FolderStatus) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('folder://status', (p) => cb(p as FolderStatus))
  return listen<FolderStatus>('folder://status', (e) => cb(e.payload))
}

export interface FolderSynced {
  pairId: string
  direction: 'send' | 'receive'
}

/** A shared-folder transfer just completed (one file delivered or received). */
export function onFolderSynced(cb: (s: FolderSynced) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('folder-synced', (p) => cb(p as FolderSynced))
  return listen<FolderSynced>('folder-synced', (e) => cb(e.payload))
}

/** A chat message arrived (from a friend) or was just sent by us. */
export function onChatMessage(cb: (m: ChatMessage) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('chat://message', (p) => cb(p as ChatMessage))
  return listen<ChatMessage>('chat://message', (e) => cb(e.payload))
}

export function onPairsChanged(cb: () => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('pairs://changed', () => cb())
  return listen('pairs://changed', () => cb())
}

export function onFriendsChanged(cb: () => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('friends://changed', () => cb())
  return listen('friends://changed', () => cb())
}

/** A second launch (Windows "Send with DropBeam") forwarded a file to send. */
export function onOpenFileSend(cb: (path: string) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('open-file-send', () => {})
  return listen<string>('open-file-send', (e) => cb(e.payload))
}

export function onFolderHistoryChanged(cb: (pairId: string) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('folder-history://changed', (p) => cb(p as string))
  return listen<string>('folder-history://changed', (e) => cb(e.payload))
}

export function onTransferUpdate(cb: (u: TransferUpdate) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('transfer://update', (p) => cb(p as TransferUpdate))
  return listen<TransferUpdate>('transfer://update', (e) => cb(e.payload))
}

export function onHistoryChanged(cb: () => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('history://changed', () => cb())
  return listen('history://changed', () => cb())
}

/**
 * Subscribe to OS file drag-and-drop. In Tauri v2 this yields real file PATHS.
 */
export function onFileDrop(
  onDrop: (paths: string[]) => void,
  onHover?: (hovering: boolean) => void,
): Promise<UnlistenFn> {
  if (!HAS_TAURI) return Promise.resolve(() => {})
  return getCurrentWebview().onDragDropEvent((event) => {
    const p = event.payload
    if (p.type === 'enter' || p.type === 'over') {
      onHover?.(true)
    } else if (p.type === 'drop') {
      onHover?.(false)
      if (p.paths && p.paths.length) onDrop(p.paths)
    } else {
      onHover?.(false)
    }
  })
}

export function isActive(state: TransferState): boolean {
  return (
    state === 'starting' ||
    state === 'waitingForPeer' ||
    state === 'connecting' ||
    state === 'waitingForAccept' ||
    state === 'transferring'
  )
}
