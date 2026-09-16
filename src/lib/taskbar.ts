// Taskbar / Dock transfer progress.
//  • Windows + Linux: the taskbar/launcher button fills as the transfer runs.
//  • macOS: the Downloads stack already shows a ring while the app is on screen,
//    so the DOCK ICON's bar is reserved for the case you can't see the app —
//    the window minimized into the Dock (GitHub #33/#24/#29). It clears the
//    moment the transfer finishes or the window comes back up.
import { getCurrentWindow, ProgressBarStatus } from '@tauri-apps/api/window'
import { HAS_TAURI } from './api'

const isMac = typeof navigator !== 'undefined' && /Mac/i.test(navigator.userAgent)

let last = -2
let pending = -1
let minimized = false
let checkedAt = 0

function apply(v: number) {
  if (v === last) return
  last = v
  const win = getCurrentWindow()
  if (v < 0) {
    win.setProgressBar({ status: ProgressBarStatus.None }).catch(() => {})
  } else {
    win.setProgressBar({ status: ProgressBarStatus.Normal, progress: v }).catch(() => {})
  }
}

/** Poll whether we're minimized, at most once a second — there is no minimize
 *  event to listen for, and this only runs while a transfer is live. */
function refreshMinimized() {
  const now = Date.now()
  if (now - checkedAt < 1000) return
  checkedAt = now
  getCurrentWindow()
    .isMinimized()
    .then((value) => {
      if (value === minimized) return
      minimized = value
      apply(value ? pending : -1)
    })
    .catch(() => {})
}

/** Set the app's taskbar/Dock progress to `pct` (0–100), or null to clear it. */
export function setTaskbarProgress(pct: number | null) {
  if (!HAS_TAURI) return
  pending = pct == null ? -1 : Math.max(0, Math.min(100, Math.round(pct)))
  if (!isMac) {
    apply(pending)
    return
  }
  // A finished transfer clears the Dock bar whether or not we're minimized.
  if (pending < 0) {
    checkedAt = 0
    apply(-1)
    return
  }
  refreshMinimized()
  if (minimized) apply(pending)
}
