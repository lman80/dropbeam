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
  autoDelete: boolean
  deleteMode: DeleteMode
  createdAt: number
}

export type FolderState = 'idle' | 'sending' | 'receiving' | 'waiting' | 'error'

export interface FolderStatus {
  pairId: string
  state: FolderState
  queued: number
  sendingFile: string | null
  percent: number
  detail: string | null
}

export interface PairUpdate {
  id: string
  twoWay?: boolean
  autoDelete?: boolean
  deleteMode?: DeleteMode
  peerName?: string
}

const realApi = {
  sendFiles: (paths: string[]) => invoke<TransferUpdate>('send_files', { paths }),
  receiveFiles: (code: string) => invoke<TransferUpdate>('receive_files', { code }),
  cancelTransfer: (id: string) => invoke<void>('cancel_transfer', { id }),
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
  createPair: (folder: string, twoWay: boolean) =>
    invoke<{ pair: Pair; invite: string }>('create_pair', { folder, twoWay }),
  acceptPair: (invite: string, folder: string) =>
    invoke<Pair>('accept_pair', { invite, folder }),
  listPairs: () => invoke<Pair[]>('list_pairs'),
  updatePair: (u: PairUpdate) =>
    invoke<Pair>('update_pair', {
      id: u.id,
      twoWay: u.twoWay,
      autoDelete: u.autoDelete,
      deleteMode: u.deleteMode,
      peerName: u.peerName,
    }),
  removePair: (id: string) => invoke<void>('remove_pair', { id }),
  pairInvite: (id: string) => invoke<string>('pair_invite', { id }),
  getFolderStatuses: () => invoke<FolderStatus[]>('get_folder_statuses'),
}

export const api: typeof realApi = HAS_TAURI ? realApi : (mockApi as typeof realApi)

export function onFolderStatus(cb: (s: FolderStatus) => void): Promise<UnlistenFn> {
  if (!HAS_TAURI) return mockListen('folder://status', (p) => cb(p as FolderStatus))
  return listen<FolderStatus>('folder://status', (e) => cb(e.payload))
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
    state === 'transferring'
  )
}
