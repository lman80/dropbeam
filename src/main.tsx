/* eslint-disable react-refresh/only-export-components -- entry point, not a component module */
import { createRoot } from 'react-dom/client'
import './index.css'
import App from './App'
import { Popover } from './windows/Popover'
import { Hud } from './windows/Hud'
import { HAS_TAURI } from './lib/api'

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
if (label === 'popover' || label === 'hud') {
  document.documentElement.classList.add('overlay-window', `window-${label}`)
}

// Apply the OS theme immediately to avoid a flash; App refines it from settings.
if (window.matchMedia('(prefers-color-scheme: dark)').matches) {
  document.documentElement.classList.add('dark')
}

const Root = label === 'popover' ? Popover : label === 'hud' ? Hud : App
createRoot(document.getElementById('root')!).render(<Root />)
