import { Monitor, Moon, Sun } from 'lucide-react'
import { useStore } from '../store'
import { MOBILE_UI } from '../lib/platform'
import { BeamLogo } from './bits'

const ORDER = ['system', 'light', 'dark'] as const

/** Cycles the appearance: match system → light → dark. */
export function ThemeToggle() {
  const theme = useStore((s) => s.settings?.theme ?? 'system')
  const save = useStore((s) => s.saveSettings)
  const cycle = () => {
    const i = ORDER.indexOf(theme)
    save({ theme: ORDER[(i + 1) % ORDER.length] })
  }
  const Icon = theme === 'dark' ? Moon : theme === 'light' ? Sun : Monitor
  return (
    <button
      className="icon-btn no-drag"
      onClick={cycle}
      title={`Appearance: ${theme === 'system' ? 'match system' : theme} (click to change)`}
      aria-label={`Appearance: ${theme}. Click to change.`}
    >
      <Icon size={17} />
    </button>
  )
}

/** macOS window chrome: an overlay title bar strip that clears the traffic
 *  lights and drags the window. (Linux/Windows draw a native title bar, so the
 *  brand + appearance toggle live at the top of the sidebar there instead.) */
export function TitleBar() {
  return (
    <div
      // Tauri v2's drag works off this ATTRIBUTE, not the CSS `app-region` (which
      // the macOS WKWebview ignores — that's why the window wouldn't drag). The
      // inner label has pointer-events:none so a drag started over the logo/title
      // still hits this region; the theme button stays clickable.
      data-tauri-drag-region
      className="titlebar-drag app-titlebar"
      style={MOBILE_UI ? {
        // iOS has no traffic lights but does have a status bar / notch to clear.
        height: 'auto',
        minHeight: 'calc(46px + env(safe-area-inset-top))',
        paddingTop: 'env(safe-area-inset-top)',
        paddingLeft: 'calc(16px + env(safe-area-inset-left))',
        paddingRight: 'calc(10px + env(safe-area-inset-right))',
      } : undefined}
    >
      <div className="app-brand">
        <BeamLogo size={19} />
        <span>DropBeam</span>
      </div>
      <ThemeToggle />
    </div>
  )
}

/** Brand row at the top of the sidebar on Linux/Windows (no overlay title bar). */
export function SidebarBrand() {
  return (
    <div className="sidebar-brand">
      <div className="app-brand">
        <BeamLogo size={20} />
        <span className="nav-label">DropBeam</span>
      </div>
      <ThemeToggle />
    </div>
  )
}
