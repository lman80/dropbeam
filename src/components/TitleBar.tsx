import { Monitor, Moon, Sun } from 'lucide-react'
import { useStore } from '../store'
import { MOBILE_UI } from '../lib/platform'
import { BeamLogo } from './bits'

const ORDER = ['system', 'light', 'dark'] as const

export function TitleBar() {
  const theme = useStore((s) => s.settings?.theme ?? 'system')
  const save = useStore((s) => s.saveSettings)

  const cycle = () => {
    const i = ORDER.indexOf(theme)
    save({ theme: ORDER[(i + 1) % ORDER.length] })
  }

  const Icon = theme === 'dark' ? Moon : theme === 'light' ? Sun : Monitor

  return (
    <div
      // Tauri v2's drag works off this ATTRIBUTE, not the CSS `app-region` (which
      // the macOS WKWebview ignores — that's why the window wouldn't drag). The
      // inner label has pointer-events:none so a drag started over the logo/title
      // still hits this region; the theme button stays clickable.
      data-tauri-drag-region
      className="titlebar-drag"
      style={{
        // Desktop keeps the fixed 46px bar with room for the macOS traffic
        // lights. iOS has neither, but does have a status bar / notch to clear.
        height: MOBILE_UI ? undefined : 46,
        minHeight: MOBILE_UI ? 'calc(46px + env(safe-area-inset-top))' : 46,
        paddingTop: MOBILE_UI ? 'env(safe-area-inset-top)' : undefined,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        paddingLeft: MOBILE_UI ? 'calc(16px + env(safe-area-inset-left))' : 80,
        paddingRight: MOBILE_UI ? 'calc(10px + env(safe-area-inset-right))' : 12,
        flexShrink: 0,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 9, pointerEvents: 'none' }}>
        <BeamLogo size={19} />
        <span style={{ fontWeight: 750, letterSpacing: '-0.01em', fontSize: 15 }}>DropBeam</span>
      </div>
      <button
        className="icon-btn no-drag"
        onClick={cycle}
        title={`Appearance: ${theme}`}
        aria-label="Toggle appearance"
      >
        <Icon size={17} />
      </button>
    </div>
  )
}
