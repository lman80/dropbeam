// SuperFeedback (lman80/SuperFeedback) on the desktop: one place that starts the
// vendored web widget, mirrors its "show feedback button" preference to React,
// and reports moments of value. The iOS app runs the native Swift widget, so
// everything here is a no-op inside the iOS webview (it's hidden anyway).
import { useSyncExternalStore } from 'react'
import { SuperFeedback } from '../vendor/superfeedback'
import { IS_IOS } from './platform'

const BACKEND = 'https://superfeedback.ashton-mcp-worker.workers.dev'
const REPO = 'lman80/dropbeam'

let started = false
let version: string | undefined
let crashReports = true
const listeners = new Set<() => void>()
const notify = () => listeners.forEach((l) => l())

/** Start the widget once, in the main window only. Crash reports follow
 *  Settings → Privacy → Share diagnostics (opt-out), like the iOS app. */
export function startFeedback(appVersion?: string, shareDiagnostics = true) {
  if (IS_IOS || started) return
  started = true
  version = appVersion
  crashReports = shareDiagnostics
  init()
}

/** Share diagnostics was switched: re-init so crash capture follows it. */
export function setFeedbackCrashReports(on: boolean) {
  if (IS_IOS || !started || on === crashReports) return
  crashReports = on
  init()
}

function init() {
  SuperFeedback.init({
    backendUrl: BACKEND,
    repo: REPO,
    app: 'DropBeam',
    // No floating button (it overlapped the Send control): the sidebar's Feedback
    // item opens the panel, and follows the "Show feedback button" setting.
    trigger: 'none',
    appVersion: version,
    theme: 'auto',
    // No console/network/click breadcrumbs: they can carry file and friend names.
    captureLogs: false,
    // Unhandled UI errors, sent on the next launch.
    captureCrashes: crashReports,
    // Ideas & roadmap (votes). Support shows only once the backend has a support link.
    community: true,
    // Default DOM-snapshot capture (no native plugin): avoids a macOS Screen
    // Recording prompt, and the webview IS the app UI.
  })
  notify()
}

function subscribe(listener: () => void) {
  listeners.add(listener)
  return () => listeners.delete(listener)
}
const enabledSnapshot = () => !IS_IOS && SuperFeedback.isEnabled()

/** The persisted "Show feedback button" preference (the widget stores it). */
export function useFeedbackButton(): [boolean, (on: boolean) => void] {
  const on = useSyncExternalStore(subscribe, enabledSnapshot)
  return [on, (next) => { SuperFeedback.setEnabled(next); notify() }]
}

export const openFeedback = () => { if (!IS_IOS) void SuperFeedback.open() }
export const openIdeas = () => { if (!IS_IOS) void SuperFeedback.openIdeas() }
export const openSupport = () => { if (!IS_IOS) void SuperFeedback.openSupport() }

/** A completed, successful outcome the user can see (drives the rare support ask). */
export function feedbackMoment(name: 'files-sent' | 'files-received' | 'chat-file-delivered' | 'friend-added') {
  if (!IS_IOS && started) SuperFeedback.moment(name)
}

/** Whether the backend offers a support link yet (Settings refreshes it on open). */
export async function fetchSupportAvailable(): Promise<boolean | null> {
  const voter = SuperFeedback.voterId()
  if (IS_IOS || !voter) return null
  try {
    const res = await fetch(`${BACKEND}/community?repo=${encodeURIComponent(REPO)}&voter=${encodeURIComponent(voter)}`)
    if (!res.ok) return null
    const body = (await res.json()) as { fund?: { supportUrl?: string | null } }
    return !!body.fund?.supportUrl
  } catch {
    return null
  }
}
