import { Monitor, Moon, Sun } from 'lucide-react'
import { useStore } from '../store'
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
      className="titlebar-drag"
      style={{
        height: 46,
        display: 'flex',
        alignItems: 'center',
        justifyContent: 'space-between',
        paddingLeft: 80,
        paddingRight: 12,
        flexShrink: 0,
      }}
    >
      <div style={{ display: 'flex', alignItems: 'center', gap: 9 }}>
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
