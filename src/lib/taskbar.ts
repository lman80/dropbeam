// Taskbar / Dock transfer progress.
//  • Windows + Linux: the taskbar/launcher button fills as the transfer runs.
//  • macOS: the Downloads stack already shows a ring while the app is on screen,
//    so the DOCK ICON's bar is reserved for the case you can't see any progress
//    on screen — the main window minimized/hidden AND the transfer card
//    minimized into the Dock or not up (GitHub #33/#24/#29). It clears the
//    moment the transfer finishes or a window comes back up.
import { getCurrentWindow, ProgressBarStatus, Window } from '@tauri-apps/api/window'
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

/** True when `win` is up on screen (visible and not minimized into the Dock). */
async function onScreen(win: Window | null): Promise<boolean> {
  if (!win) return false
  const [visible, min] = await Promise.all([win.isVisible(), win.isMinimized()])
  return visible && !min
}

/** Poll whether any progress is on screen, at most once a second — there is no
 *  minimize event to listen for, and this only runs while a transfer is live.
 *  (`minimized` = "nothing showing progress is on screen".) */
function refreshMinimized() {
  const now = Date.now()
  if (now - checkedAt < 1000) return
  checkedAt = now
  Promise.all([onScreen(getCurrentWindow()), Window.getByLabel('receive').then(onScreen, () => false)])
    .then(([main, card]) => !main && !card)
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
