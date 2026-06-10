// Windows/Linux taskbar progress — the cross-platform counterpart to macOS's
// Downloads-stack progress (NSProgress, see download_progress.rs). macOS already
// shows transfer progress on the Dock's Downloads stack, so we only drive the
// taskbar/launcher here; on macOS this is a no-op to avoid a duplicate indicator.
import { getCurrentWindow, ProgressBarStatus } from '@tauri-apps/api/window'
import { HAS_TAURI } from './api'

const isMac = typeof navigator !== 'undefined' && /Mac/i.test(navigator.userAgent)

let last = -2
/** Set the app's taskbar progress to `pct` (0–100), or null to clear it. */
export function setTaskbarProgress(pct: number | null) {
  if (!HAS_TAURI || isMac) return
  const v = pct == null ? -1 : Math.max(0, Math.min(100, Math.round(pct)))
  if (v === last) return
  last = v
  const win = getCurrentWindow()
  if (v < 0) {
    win.setProgressBar({ status: ProgressBarStatus.None }).catch(() => {})
  } else {
    win.setProgressBar({ status: ProgressBarStatus.Normal, progress: v }).catch(() => {})
  }
}
