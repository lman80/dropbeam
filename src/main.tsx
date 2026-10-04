/* eslint-disable react-refresh/only-export-components -- entry point, not a component module */
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App'
import { Popover } from './windows/Popover'
import { Hud } from './windows/Hud'
import { ReceiveCard } from './windows/ReceiveCard'
import { api, HAS_TAURI } from './lib/api'
import { DESKTOP_OS, IS_IOS, MOBILE_UI } from './lib/platform'
import { setFeedbackCrashReports, startFeedback } from './lib/feedback'
import { ErrorBoundary } from './components/ErrorBoundary'
import { useStore } from './store'

// Which window are we? The popover and HUD load the same bundle as the main
// app and pick their compact UI from the Tauri window label. `?window=` lets us
// preview those surfaces in a plain browser.
function windowLabel(): string {
  const forced = new URLSearchParams(location.search).get('window')
  if (forced) return forced
  if (HAS_TAURI) {
    try {
      const internals = (window as unknown as {
        __TAURI_INTERNALS__?: { metadata?: { currentWindow?: { label?: string } } }
      }).__TAURI_INTERNALS__
      return internals?.metadata?.currentWindow?.label ?? 'main'
    } catch {
      return 'main'
    }
  }
  return 'main'
}

const label = windowLabel()

// The popover/HUD are transparent windows — the body must not paint a background.
if (label === 'popover' || label === 'hud' || label === 'receive') {
  document.documentElement.classList.add('overlay-window', `window-${label}`)
}

// iOS: one class on <html> gates every phone-only style rule in index.css, so
// the desktop cascade is untouched. Set before the first paint (no reflow).
if (MOBILE_UI) {
  document.documentElement.classList.add('mobile')
} else {
  // Desktop: one class picks platform chrome (titlebar inset, scrollbars, fonts).
  document.documentElement.classList.add(`platform-${DESKTOP_OS}`)
}

// Apply the OS theme immediately to avoid a flash; App refines it from settings.
if (window.matchMedia('(prefers-color-scheme: dark)').matches) {
  document.documentElement.classList.add('dark')
}

// IME: WebKit delivers the Enter that CONFIRMS a Chinese/Japanese composition as
// a keydown with keyCode 229 — which would also submit the surrounding form,
// sending half-typed text. Swallow just that keydown for form fields.
window.addEventListener('keydown', (e) => {
  if (e.key !== 'Enter' || e.keyCode !== 229 || e.isComposing) return
  const t = e.target as HTMLElement | null
  if (t && t.tagName === 'INPUT' && (t as HTMLInputElement).form) e.preventDefault()
}, true)

// Browser preview only: expose the store so screenshot scripts can reach every state.
if (!HAS_TAURI) (window as unknown as { __store?: typeof useStore }).__store = useStore

const Root =
  label === 'popover' ? Popover : label === 'hud' ? Hud : label === 'receive' ? ReceiveCard : App
async function renderApp() {
  if (MOBILE_UI) await import('./mobile.css')
  createRoot(document.getElementById('root')!).render(
    <ErrorBoundary region={`window:${label}`}><Root /></ErrorBoundary>,
  )
}
void renderApp()

// SuperFeedback (src/lib/feedback.ts): screenshots the app, takes a message, and
// opens a GitHub Issue in DropBeam's OWN repo via the backend Worker. Main window
// only (not the popover/HUD), and never on iOS, which runs the native widget.
if (label === 'main' && !IS_IOS) {
  void (async () => {
    let appVersion: string | undefined
    let shareDiagnostics = true
    if (HAS_TAURI) {
      try {
        appVersion = await (await import('@tauri-apps/api/app')).getVersion()
      } catch {
        /* version is best-effort */
      }
      try {
        shareDiagnostics = (await api.getSettings()).shareDiagnostics !== false
      } catch {
        /* default on, like the setting */
      }
    }
    startFeedback(appVersion, shareDiagnostics)
    useStore.subscribe((s) => { if (s.settings) setFeedbackCrashReports(s.settings.shareDiagnostics !== false) })
  })()
}

// Capture uncaught errors into the native log file so a startup problem on a
// machine we can't reach (e.g. a tester's Windows box) leaves a trace.
if (HAS_TAURI) {
  api.frontendLog(`ui: booting window=${label}`).catch(() => {})
  window.addEventListener('error', (e) =>
    api.frontendLog(`ui error: ${e.message} @ ${e.filename}:${e.lineno}`).catch(() => {}),
  )
  window.addEventListener('unhandledrejection', (e) =>
    api.frontendLog(`ui unhandledrejection: ${String(e.reason)}`).catch(() => {}),
  )
}
