import { FolderSync, History, Send, Settings, Users } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { isActive } from '../lib/api'
import { useStore, type View } from '../store'

const NAV: { id: View; label: string; icon: LucideIcon }[] = [
  { id: 'send', label: 'Send & Receive', icon: Send },
  { id: 'friends', label: 'Friends', icon: Users },
  { id: 'folders', label: 'Shared Folders', icon: FolderSync },
  { id: 'history', label: 'History', icon: History },
  { id: 'settings', label: 'Settings', icon: Settings },
]

function initials(name: string): string {
  const parts = name.trim().split(/\s+/).filter(Boolean)
  if (!parts.length) return '○'
  if (parts.length === 1) return parts[0].slice(0, 2).toUpperCase()
  return (parts[0][0] + parts[parts.length - 1][0]).toUpperCase()
}

export function Sidebar() {
  const view = useStore((s) => s.view)
  const setView = useStore((s) => s.setView)
  const name = useStore((s) => s.settings?.displayName ?? '')
  const activeCount = useStore(
    (s) => Object.values(s.transfers).filter((t) => isActive(t.state)).length,
  )

  return (
    <nav
      style={{
        width: 218,
        padding: '6px 12px 12px',
        display: 'flex',
        flexDirection: 'column',
        gap: 3,
        flexShrink: 0,
      }}
    >
      {NAV.map((item) => {
        const active = view === item.id
        const Icon = item.icon
        return (
          <button
            key={item.id}
            data-testid={`nav-${item.id}`}
            className={`nav-item${active ? ' active' : ''}`}
            onClick={() => setView(item.id)}
          >
            <Icon size={18} strokeWidth={2.1} />
            <span style={{ flex: 1 }}>{item.label}</span>
            {item.id === 'send' && activeCount > 0 && (
              <span
                className="chip"
                style={{
                  background: 'var(--accent)',
                  color: 'white',
                  minWidth: 20,
                  justifyContent: 'center',
                  padding: '1px 6px',
                }}
              >
                {activeCount}
              </span>
            )}
          </button>
        )
      })}

      <div style={{ flex: 1 }} />

      <div
        style={{
          display: 'flex',
          alignItems: 'center',
          gap: 10,
          padding: '10px',
          marginTop: 8,
          borderTop: '1px solid var(--border)',
        }}
      >
        <div
          style={{
            width: 34,
            height: 34,
            borderRadius: 10,
            display: 'grid',
            placeItems: 'center',
            color: 'white',
            fontWeight: 700,
            fontSize: 13,
            background: 'linear-gradient(135deg, var(--accent), var(--accent-2))',
            flexShrink: 0,
          }}
        >
          {initials(name)}
        </div>
        <div style={{ overflow: 'hidden' }}>
          <div
            style={{
              fontSize: 13,
              fontWeight: 650,
              whiteSpace: 'nowrap',
              textOverflow: 'ellipsis',
              overflow: 'hidden',
            }}
          >
            {name || 'This device'}
          </div>
          <div style={{ fontSize: 11, color: 'var(--text-faint)' }}>This device</div>
        </div>
      </div>
    </nav>
  )
}
