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
export type Locality = 'unknown' | 'local' | 'direct' | 'internet'

/** Live detail of how two peers are connected — the connection inspector data. */
export interface ConnDetail {
  /** "local" | "direct" | "relay" | "connecting" */
  path: string
  /** Round-trip latency in ms for the active path (null if not measured). */
  rttMs: number | null
  /** True when on relay but a direct path is actively forming (hole-punch). */
  upgrading: boolean
  /** Short relay region/host (e.g. "use1") when relayed, else null. */
  relay: string | null
}

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
  /** Live connection detail (path, rtt, relay, upgrading) for the inspector. */
  connDetail?: ConnDetail | null
  /** A short reason for a PARKED/waiting state (e.g. "Waiting for a direct connection"). */
  detail?: string | null
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
  /** Cap internet upload speed (Mbps) to leave headroom; 0 = unlimited. */
  uploadLimitMbps: number
  /** Show speeds in megabits/sec (Mbps) instead of megabytes/sec (MB/s). */
  showMegabits: boolean
  /** Only send over a direct path (local/p2p) — fail rather than use the relay. */
  requireDirect: boolean
  /** Wait for a direct connection: hold transfers off the slow relay until a direct
   *  path forms (park, don't fail). Friend sends show a "Send over relay anyway"
   *  escape; folder sync waits much longer for direct before settling for relay. */
  waitForDirect: boolean
  /** Fan one big file across several QUIC streams for speed (reassembled
   *  byte-identically). On by default; a kill-switch for flaky networks. */
  parallelStreams: boolean
  /** Absolute path to the user's profile picture (in the config dir). '' = none. */
  avatar: string
  /** Pop a native notification when a chat arrives & the app isn't focused. */
  notifyOnMessage: boolean
  /** Send read receipts so friends see when you've read their message. */
  sendReadReceipts: boolean
  /** Free Giphy API key (developers.giphy.com) powering GIF search. '' = off. */
  giphyApiKey: string
  /** Detailed diagnostics logging (app + iroh internals). Applied on restart. */
  verboseLogging: boolean
  /** Show the floating "syncing folder…" popup during shared-folder transfers. */
  showSyncPopup: boolean
  /** Upload a redacted error/perf digest ~daily so the developer can fix issues
   *  users never see. Never sends file names or contents. On by default. */
  shareDiagnostics: boolean
  /** Where diagnostics go — the operator's own collector URL (empty = send
   *  nowhere). Keeps the destination operator-controlled, never baked in. */
  diagnosticsUrl: string
  /** Lab Mode: let ONE trusted operator device run automated tests and push
   *  updates over the encrypted link. Off by default; even on, only the exact
   *  operator id below is accepted. */
  labModeEnabled: boolean
  /** The node id of the sole device allowed to drive Lab Mode. Empty = nothing
   *  is accepted even when enabled. */
  labOperatorId: string
  /** How long a mirror folder keeps deleted/replaced copies before auto-removing
   *  them. 0 = keep forever. Default 30 days. */
  folderHistoryKeepDays: number
  /** Per-folder disk budget (bytes) for recoverable copies; oldest evicted first.
   *  0 = no limit. Default 2 GiB. */
  folderHistoryBudgetBytes: number
}

/** Per-folder rollup of recovery-history disk usage, for the storage view. */
export interface FolderHistorySummary {
  pairId: string
  folderName: string
  folder: string
  bytes: number
  itemCount: number
  oldestMs: number | null
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
  /** Multi-person folders: all pairwise links of one shared folder share this id.
   * Null = a classic 1:1 folder. */
  groupId: string | null
  /** This device is a read-only viewer on this folder (we never send). */
  iAmViewer?: boolean
  /** The peer on this link is a read-only viewer (we never receive from them). */
  peerIsViewer?: boolean
  /** The folder OWNER's (creator's) endpoint id. We are the owner iff this equals
   * our own endpoint id. Only the owner may assign roles. Null on legacy folders. */
  ownerEid?: string | null
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
  /** Local path to the friend's profile picture (received over the wire). */
  avatar: string | null
  /** True once you've renamed this friend locally (their broadcasts won't override). */
  nameCustom?: boolean
}

/** A GIF attachment on a chat message (Giphy). */
export interface GifMeta {
  provider: string
  id: string
  url: string
  page: string
  w: number
  h: number
}

/** One emoji reaction on a message (from me or the friend). */
export interface Reaction {
  emoji: string
  fromMe: boolean
}

/** One message in a chat with a friend. */
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
  /** Delivery state for messages WE sent. 'sent' is tolerated from older builds. */
  status: 'sending' | 'delivered' | 'read' | 'failed' | 'sent' | null
  ts: number
  /** Logical ordering clock — sort by this (then ts, then id), not wall-clock. */
  seq: number
  /** Reply/quote: id of the message this replies to + a cached one-line preview. */
  replyTo?: string | null
  replyPreview?: string | null
  /** Emoji reactions (a set keyed by from-me + emoji). */
  reactions: Reaction[]
  /** The author edited the text. */
  edited: boolean
  /** The author unsent it (render a tombstone). */
  deleted: boolean
  /** A GIF attachment — render a GIF bubble when present. */
  gif?: GifMeta | null
  /** UI-only (never persisted, never on the wire): set on the SENDER's file card when
   *  the byte transfer it describes ultimately failed, so the card can offer "tap to
   *  resend" instead of implying the file arrived. Carries the failed transfer id so
   *  the resend reuses the dedup-safe retryTransfer. */
  fileXferFailed?: boolean
  fileXferId?: string
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
  /** The peer stopped sharing this folder on their side. */
  peerUnshared?: boolean
  /** File names still waiting to send (active one excluded) — drives the per-file
   *  drop list instead of one-at-a-time popups. */
  queuedFiles?: string[]
  /** How many files the peer has (from its last reconcile) — for the "both have N
   *  files, in sync" indicator. */
  peerFiles?: number
  /** Aggregate progress for the current send burst (a folder drop): total files
   *  and how many are done, so the HUD shows one "12 of 50" bar. */
  sessionTotalFiles?: number
  sessionDoneFiles?: number
  /** Sync is paused for this folder (a shared switch — either member set it). */
  paused?: boolean
  /** Live connection detail for the active folder transfer. */
  connDetail?: ConnDetail | null
}

/** The honest answer to "are these two folders identical?" from `verifyFolder`.
 *  "Identical" = same set of relative paths, each the same byte size (mtime is not
 *  compared — two machines round it differently; that's the same rule the sync uses
 *  to decide a file is in sync). Any genuine difference is counted and being fixed. */
export interface VerifyResult {
  /** false when we couldn't reach the peer (offline / no fresh snapshot in time). */
  compared: boolean
  /** true only when the two folders are byte-size identical across every path. */
  identical: boolean
  /** Files present on both sides with the same size (the agreeing core). */
  matched: number
  /** Total differences the reconcile will fix (the number shown to the user). */
  differences: number
  /** Files we have that the peer is missing / has a different-size copy of. */
  missingOnPeer: number
  /** Files the peer has that we're missing / have a different-size copy of. */
  missingLocally: number
  /** Pending deletes still propagating either way. */
  pendingDeletes: number
  /** Our current file count. */
  localFiles: number
  /** The peer's current file count from the snapshot we compared against. */
  peerFiles: number
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
  /** Pick an image and set it as the profile picture. Returns updated settings. */
  setProfileAvatar: () => invoke<Settings>('set_profile_avatar'),
  clearProfileAvatar: () => invoke<Settings>('clear_profile_avatar'),
  revealPath: (path: string) => invoke<void>('reveal_path', { path }),
  openPath: (path: string) => invoke<void>('open_path', { path }),
  /** Bundle all logs + a redacted header into one .txt in Downloads; returns its path. */
  exportDiagnostics: () => invoke<string>('export_diagnostics'),
  /** Send one diagnostics digest now to verify the endpoint. Returns a summary. */
  diagnosticsTest: () => invoke<string>('diagnostics_test'),
  /** Relaunch the app (to apply the verbose-logging toggle). */
  restartApp: () => invoke<void>('restart_app'),
  /** True when transfers are stuck on the relay despite a LAN peer (Local Network perm likely off). */
  lanNetworkBlocked: () => invoke<boolean>('lan_network_blocked'),
  /** Open System Settings → Privacy & Security → Local Network. */
  openLocalNetworkSettings: () => invoke<void>('open_local_network_settings'),
  openUrl: (url: string) => invoke<void>('open_url', { url }),
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
  /** Owner: make a folder member view-only (read-only) or an editor again. */
  setMemberRole: (id: string, viewer: boolean) => invoke<void>('set_member_role', { id, viewer }),
  /** This device's own iroh endpoint id (null until iroh is up). Used to tell
   * whether we own a folder (pair.ownerEid === this) before showing role controls. */
  myEndpointId: () => invoke<string | null>('my_endpoint_id'),
  verifyFolders: () => invoke<void>('verify_folders'),
  /** Re-exchange manifests for ONE shared folder and report a trustworthy answer:
   *  are the two folders identical, how many files match, how many differ (and are
   *  being fixed). Reuses the existing reconcile plumbing. `pairId` is the link. */
  verifyFolder: (pairId: string) => invoke<VerifyResult>('verify_folder', { pairId }),
  stopFolderTransfer: (pairId: string) => invoke<void>('stop_folder_transfer', { pairId }),
  /** Pause or resume sync for a shared folder (a shared switch — flips both sides). */
  setFolderPaused: (pairId: string, paused: boolean) =>
    invoke<void>('set_folder_paused', { pairId, paused }),
  /** Delete abandoned transfer leftovers (paused/failed partials). Returns bytes freed. */
  clearTransferCache: () => invoke<number>('clear_transfer_cache'),
  /** Tell the backend a transfer card is on screen (true) or gone (false) — drives the transient Dock icon. */
  setCardActive: (active: boolean) => invoke<void>('set_card_active', { active }),
  pairInvite: (id: string) => invoke<string>('pair_invite', { id }),
  /** Invite another person into an existing folder (makes it a 3+ person group). */
  folderAddPerson: (id: string) => invoke<string>('folder_add_person', { id }),
  /** Invite an existing friend straight into a shared folder (no code) — they get a
   *  prompt to accept + pick a folder. `pairId` is any link of the folder/group. */
  inviteFriendToFolder: (pairId: string, friendId: string, code?: string | null) =>
    invoke<void>('invite_friend_to_folder', { pairId, friendId, code: code ?? null }),
  getFolderStatuses: () => invoke<FolderStatus[]>('get_folder_statuses'),
  listFolderHistory: (pairId: string) =>
    invoke<HistoryItem[]>('list_folder_history', { pairId }),
  restoreFolderItem: (pairId: string, itemId: string) =>
    invoke<void>('restore_folder_item', { pairId, itemId }),
  forgetFolderItem: (pairId: string, itemId: string) =>
    invoke<void>('forget_folder_item', { pairId, itemId }),
  /** Disk used by recoverable copies, per shared folder. */
  folderHistorySummary: () =>
    invoke<FolderHistorySummary[]>('folder_history_summary'),
  /** Wipe one folder's recoverable copies. Returns bytes freed. */
  clearFolderHistory: (pairId: string) =>
    invoke<number>('clear_folder_history', { pairId }),
  /** Wipe recoverable copies across every shared folder. Returns bytes freed. */
  clearAllFolderHistory: () => invoke<number>('clear_all_folder_history'),
  // Friends — named peers you send to directly.
  createFriend: (friendName: string) =>
    invoke<{ friend: Friend; invite: string }>('create_friend', { friendName }),
  acceptFriend: (invite: string) => invoke<Friend>('accept_friend', { invite }),
  listFriends: () => invoke<Friend[]>('list_friends'),
  renameFriend: (id: string, name: string) =>
    invoke<void>('rename_friend', { id, name }),
  removeFriend: (id: string) => invoke<void>('remove_friend', { id }),
  pingFriend: (id: string) => invoke<boolean>('ping_friend', { id }),
  /** Probe how we're connected to a friend right now (connection inspector). */
  probeConnection: (friendId: string) =>
    invoke<ConnDetail | null>('probe_connection', { friendId }),
  /** "Send over relay anyway" — break the wait-for-direct park for this transfer/folder. */
  forceRelay: (id: string) => invoke<void>('force_relay', { id }),
  setFriendAutoAccept: (id: string, autoAccept: boolean) =>
    invoke<void>('set_friend_auto_accept', { id, autoAccept }),
  respondToOffer: (id: string, accept: boolean, dest?: string) =>
    invoke<void>('respond_to_offer', { id, accept, dest: dest ?? null }),
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
  sendChatMessage: (
    friendId: string,
    text: string,
    replyTo?: string | null,
    replyPreview?: string | null,
  ) =>
    invoke<ChatMessage>('send_chat_message', {
      friendId,
      text,
      replyTo: replyTo ?? null,
      replyPreview: replyPreview ?? null,
    }),
  sendChatFileNote: (friendId: string, names: string[], bytes: number, paths: string[]) =>
    invoke<ChatMessage>('send_chat_file_note', { friendId, names, bytes, paths }),
  /** Save an image pasted into the chat composer to an app-managed folder (bounded
   *  to the last 50 pastes) and return its path for the staged-file send flow.
   *  Takes base64 — a raw byte array would serialize as a huge JSON number[]. */
  savePastedImage: (b64: string, ext: string) =>
    invoke<string>('save_pasted_image', { b64, ext }),
  /** Add/remove an emoji reaction on a message (ours or theirs). */
  reactToMessage: (friendId: string, messageId: string, emoji: string, add: boolean) =>
    invoke<void>('react_to_message', { friendId, messageId, emoji, add }),
  /** Edit the text of a message we sent. */
  editChatMessage: (friendId: string, messageId: string, text: string) =>
    invoke<void>('edit_chat_message', { friendId, messageId, text }),
  /** Unsend a message we sent (tombstone on both sides). */
  deleteChatMessage: (friendId: string, messageId: string) =>
    invoke<void>('delete_chat_message', { friendId, messageId }),
  /** Tell a friend whether we're typing (ephemeral, online-only). */
  sendTyping: (friendId: string, on: boolean) => invoke<void>('send_typing', { friendId, on }),
  /** Send a read receipt: seen everything up to `upTo` (ms). Honors the toggle. */
  sendReadReceipt: (friendId: string, upTo: number) =>
    invoke<void>('send_read_receipt', { friendId, upTo }),
  /** Download a GIF's bytes (Giphy CDN) to a temp file; returns the local path. */
  downloadGif: (url: string, id: string) => invoke<string>('download_gif', { url, id }),
  /** Drop a GIF card in the thread (bytes already sent via the file transfer). */
  sendChatGif: (
    friendId: string,
    name: string,
    bytes: number,
    path: string,
    gif: GifMeta,
  ) => invoke<ChatMessage>('send_chat_gif', { friendId, name, bytes, path, gif }),
  /** Tell Rust which chat is open (so it won't notify for the one on screen). */
  setActiveChat: (peerId: string | null) => invoke<void>('set_active_chat', { peerId }),
  /** Reflect total unread on the Dock/taskbar badge. */
  setUnreadBadge: (count: number) => invoke<void>('set_unread_badge', { count }),
}

export const api: typeof realApi = HAS_TAURI ? realApi : (mockApi as typeof realApi)

export function onFolderStatus(cb: (s: FolderStatus) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('folder://status', (p) => cb(p as FolderStatus))
  return listen<FolderStatus>('folder://status', (e) => cb(e.payload))
}

export interface FolderSynced {
  pairId: string
  direction: 'send' | 'receive'
  /** Files still queued for this folder after this one — 0 means the whole drop
   *  finished, so the UI can play the sound ONCE instead of once per file. */
  remaining?: number
  /** Relative names of the files moved in this batch — for the chat timeline rows
   *  (GitHub #23). Absent on older engines. */
  files?: string[]
  /** Total bytes moved in this batch. */
  bytes?: number
  /** Who these files came from (your saved label for them, else their broadcast
   *  name). Only present on the receive side; absent on send and older engines. */
  from?: string
  /** What happened: 'added' (default — files synced) or 'moved' (files relocated
   *  within the folder). Absent on older engines → treated as 'added'. */
  action?: 'added' | 'moved'
  /** For action='moved': the from→to relative paths of each relocated file. */
  moves?: { from: string; to: string }[]
}

/** A shared-folder transfer just completed (one file delivered or received). */
export function onFolderSynced(cb: (s: FolderSynced) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('folder-synced', (p) => cb(p as FolderSynced))
  return listen<FolderSynced>('folder-synced', (e) => cb(e.payload))
}

/** A whole shared-folder drop finished — summary stats for the folder card,
 *  mirroring the Send/Receive tab's completion line (size, time, avg speed). */
export interface FolderComplete {
  pairId: string
  direction: 'send' | 'receive'
  files: number
  bytes: number
  durationMs: number
  avgBps: number
}

export function onFolderComplete(cb: (s: FolderComplete) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('folder-complete', (p) => cb(p as FolderComplete))
  return listen<FolderComplete>('folder-complete', (e) => cb(e.payload))
}

/** A friend invited us directly into a shared folder (they picked us when creating
 *  it). We show a prompt to accept + choose a local folder. */
export interface FolderInvite {
  code: string
  folderName: string
  fromName: string
  fromId: string
}

export function onFolderInvite(cb: (i: FolderInvite) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('folder-invite://incoming', (p) => cb(p as FolderInvite))
  return listen<FolderInvite>('folder-invite://incoming', (e) => cb(e.payload))
}

/** A chat message arrived (from a friend) or was just sent by us. */
export function onChatMessage(cb: (m: ChatMessage) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('chat://message', (p) => cb(p as ChatMessage))
  return listen<ChatMessage>('chat://message', (e) => cb(e.payload))
}

/** A friend started/stopped typing to us (ephemeral). */
export interface ChatTyping {
  peerId: string
  on: boolean
}
export function onChatTyping(cb: (t: ChatTyping) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('chat://typing', (p) => cb(p as ChatTyping))
  return listen<ChatTyping>('chat://typing', (e) => cb(e.payload))
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
