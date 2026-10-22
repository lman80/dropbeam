import { invoke } from '@tauri-apps/api/core'
import { HAS_TAURI } from './api'
import { IS_LINUX, IS_WINDOWS } from './platform'

/** background_host.rs: does DropBeam start with the computer, before anyone signs in? */
export type BackgroundHostStatus = {
  supported: boolean
  enabled: boolean
  running: boolean
  /** The task starts this copy of DropBeam (false after it moved). */
  current: boolean
  /** Linux: the command to run instead of a switch. */
  command: string | null
}

export const LINUX_BACKGROUND_COMMAND = 'sudo systemctl enable --now dropbeam-server@$USER'

let mockStatus: BackgroundHostStatus = { supported: IS_WINDOWS, enabled: false, running: false, current: true, command: IS_LINUX ? LINUX_BACKGROUND_COMMAND : null }

export const backgroundHostApi = HAS_TAURI ? {
  status: () => invoke<BackgroundHostStatus>('background_host_status'),
  /** Windows asks for permission (UAC) on this computer's screen. */
  set: (enabled: boolean) => invoke<BackgroundHostStatus>('background_host_set', { enabled }),
} : {
  status: async () => mockStatus,
  set: async (enabled: boolean) => {
    mockStatus = { ...mockStatus, enabled, running: enabled }
    return mockStatus
  },
}
