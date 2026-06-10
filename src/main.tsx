/* eslint-disable react-refresh/only-export-components -- entry point, not a component module */
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App'
import { Popover } from './windows/Popover'
import { Hud } from './windows/Hud'
import { ReceiveCard } from './windows/ReceiveCard'
import { api, HAS_TAURI } from './lib/api'
import { SuperFeedback } from './vendor/superfeedback'

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

// Apply the OS theme immediately to avoid a flash; App refines it from settings.
if (window.matchMedia('(prefers-color-scheme: dark)').matches) {
  document.documentElement.classList.add('dark')
}

const Root =
  label === 'popover' ? Popover : label === 'hud' ? Hud : label === 'receive' ? ReceiveCard : App
createRoot(document.getElementById('root')!).render(<Root />)

// SuperFeedback — a floating "Send feedback" button (main window only, not the
// popover/HUD). It screenshots the app, takes a message, and opens a GitHub
// Issue in DropBeam's OWN repo via the user's backend Worker. Dynamically
// imported so it never loads in the overlay windows.
if (label === 'main') {
  void (async () => {
    let appVersion: string | undefined
    if (HAS_TAURI) {
      try {
        appVersion = await (await import('@tauri-apps/api/app')).getVersion()
      } catch {
        /* version is best-effort */
      }
    }
    SuperFeedback.init({
      backendUrl: 'https://superfeedback.ashton-mcp-worker.workers.dev',
      repo: 'lman80/dropbeam',
      app: 'DropBeam',
      // No floating button (it overlapped the Send control). We open the
      // centered panel from a "Feedback" item in the left sidebar instead.
      trigger: 'none',
      appVersion,
      // v1.2.0 redesign follows the system theme — match DropBeam's light/dark.
      theme: 'auto',
      // Default DOM-snapshot capture (no native plugin) — avoids a macOS
      // Screen-Recording permission prompt; the webview IS the app UI.
    })
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
