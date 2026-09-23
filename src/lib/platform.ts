// Which shell is this bundle rendering — the desktop windows, or the iOS app?
//
// The iOS build ships the exact same JS bundle as macOS/Windows/Linux, so every
// phone-specific tweak in the UI is keyed off this ONE flag. Anything that isn't
// behind it is, by construction, byte-identical to the desktop build.
//
// It's computed synchronously from the user agent so the very first render is
// already correct (no sidebar-then-tabbar flash). That's reliable here because
// the webview is the platform's own: WKWebView on iOS always reports
// iPhone/iPad/iPod, and no desktop webview ever does.

/** True when running inside the iOS app (iPhone or iPad). */
export const IS_IOS: boolean = (() => {
  if (typeof navigator === 'undefined') return false
  const ua = navigator.userAgent
  if (/iPhone|iPod|iPad/.test(ua)) return true
  // iPadOS 13+ can report a desktop-Safari UA. A real Mac reports 0 touch
  // points, so this separates the two without misfiring on desktop.
  return /Macintosh/.test(ua) && (navigator.maxTouchPoints ?? 0) > 1
})()

/** `?mobile=1` forces the phone layout in a plain browser, for previewing it. */
const FORCED =
  typeof location !== 'undefined' &&
  new URLSearchParams(location.search).get('mobile') === '1'

/**
 * Render the phone UI: bottom tab bar instead of the sidebar, touch-sized
 * controls, safe-area insets, and desktop-only features hidden.
 */
export const MOBILE_UI: boolean = IS_IOS || FORCED

/** Desktop OS the window runs on (for platform-specific chrome and copy). */
export type DesktopOs = 'mac' | 'windows' | 'linux'
export const DESKTOP_OS: DesktopOs = (() => {
  if (typeof navigator === 'undefined') return 'mac'
  // `?os=linux|windows|mac` previews another platform's chrome in a browser.
  const forced = typeof location !== 'undefined' ? new URLSearchParams(location.search).get('os') : null
  if (forced === 'mac' || forced === 'windows' || forced === 'linux') return forced
  const ua = navigator.userAgent
  if (/Windows/i.test(ua)) return 'windows'
  if (/Mac/i.test(ua)) return 'mac'
  return 'linux'
})()
export const IS_MAC = DESKTOP_OS === 'mac'
export const IS_WINDOWS = DESKTOP_OS === 'windows'
export const IS_LINUX = DESKTOP_OS === 'linux'

/** "menu bar" on macOS, the system tray elsewhere. */
export const TRAY_NAME = IS_MAC ? 'menu bar' : 'system tray'
