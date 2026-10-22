// Open in place + "Show Locations as a drive" (docs/locations-v2.md).
// Thin wrappers over the Rust commands in stream.rs / drive.rs.
import { invoke } from '@tauri-apps/api/core'
import { HAS_TAURI } from './api'

export type PreviewKind = 'image' | 'video' | 'audio' | 'pdf' | 'text' | 'other'
export interface ReadUrl { url: string; kind: PreviewKind; mime: string; size: number; etag: string }
export interface StreamStatus { path: 'local' | 'direct' | 'relay' | 'connecting' | string; rttMs?: number | null; bytesPerSec: number; received: number; cachedBytes: number; cacheLimit: number }
export interface DriveSettings { enabled: boolean; autoMount: boolean; cacheGb: number }
export interface DriveStatus {
  supported: boolean
  settings: DriveSettings
  mounted: boolean
  busy: boolean
  path?: string | null
  error?: string | null
  devices: number
  locations: number
  pendingUploads: number
  uploadErrors: string[]
  cachedBytes: number
  davUrl?: string | null
  windowsFix?: string | null
}

const where = (friendId: string, locationId: string, relPath: string) => ({ friendId, locationId, relPath })
const unavailable = () => Promise.reject(new Error('Opening files in place needs the DropBeam app.'))

export const streamApi = {
  readUrl: (friendId: string, locationId: string, relPath: string) =>
    HAS_TAURI ? invoke<ReadUrl>('location_read_url', where(friendId, locationId, relPath)) : unavailable(),
  thumb: (friendId: string, locationId: string, relPath: string, size: number, modified: number, max = 128) =>
    HAS_TAURI ? invoke<string | null>('location_thumb', { ...where(friendId, locationId, relPath), size, modified, max }) : Promise.resolve(null),
  open: (friendId: string, locationId: string, relPath: string) =>
    HAS_TAURI ? invoke<string>('location_open', where(friendId, locationId, relPath)) : unavailable(),
  cacheFile: (friendId: string, locationId: string, relPath: string) =>
    HAS_TAURI ? invoke<string>('location_cache_file', where(friendId, locationId, relPath)) : unavailable(),
  status: (friendId: string) => HAS_TAURI ? invoke<StreamStatus>('stream_status', { friendId }) : unavailable(),
  clearCache: () => HAS_TAURI ? invoke<number>('stream_clear_cache') : unavailable(),
}

export const driveApi = {
  status: () => HAS_TAURI ? invoke<DriveStatus>('drive_status') : unavailable(),
  mount: () => invoke<string>('drive_mount'),
  unmount: () => invoke<void>('drive_unmount'),
  refresh: () => invoke<DriveStatus>('drive_refresh'),
  setSettings: (settings: DriveSettings) => invoke<DriveStatus>('drive_settings_set', { settings }),
  /** Opens the drive, or reveals one Location item in it (mounting first if needed). */
  open: (friendId?: string, locationId?: string, relPath?: string) => invoke<string>('drive_open', { friendId, locationId, relPath }),
  fixWindows: () => invoke<DriveStatus>('drive_fix_windows'),
}

/** "Local network" / "Direct" / "Relay" in the glossary's words. */
export function pathLabel(path: string): string {
  return path === 'local' ? 'Local network' : path === 'direct' ? 'Direct' : path === 'relay' ? 'Relay' : 'Connecting'
}
