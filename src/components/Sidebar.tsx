import {
  FolderSync,
  HardDrive,
  History,
  MessageCircle,
  MessageSquarePlus,
  Send,
  Settings,
  Users,
} from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { isActive } from '../lib/api'
import { useStore, type View } from '../store'
import { FriendAvatar } from './FriendAvatar'
import { SuperFeedback } from '../vendor/superfeedback'
import { IS_MAC } from '../lib/platform'
import { SidebarBrand } from './TitleBar'

const NAV: { id: View; label: string; icon: LucideIcon }[] = [
  { id: 'send', label: 'Send & Receive', icon: Send },
  { id: 'friends', label: 'Friends', icon: Users },
  { id: 'chat', label: 'Chat', icon: MessageCircle },
  { id: 'locations', label: 'Locations', icon: HardDrive },
  { id: 'folders', label: 'Shared Folders', icon: FolderSync },
  { id: 'history', label: 'History', icon: History },
  { id: 'settings', label: 'Settings', icon: Settings },
]

export function Sidebar() {
  const view = useStore((s) => s.view)
  const setView = useStore((s) => s.setView)
  const avatar = useStore((s) => s.settings?.avatar ?? null)
  const name = useStore((s) => s.settings?.displayName ?? '')
  const activeCount = useStore(
    (s) => Object.values(s.transfers).filter((t) => isActive(t.state)).length,
  )
  const unreadCount = useStore((s) =>
    Object.values(s.chatUnread).reduce((a, b) => a + b, 0),
  )

  return (
    <nav className="app-sidebar" aria-label="Main">
      {!IS_MAC && <SidebarBrand />}
      {NAV.map((item) => {
        const active = view === item.id
        const Icon = item.icon
        const badge = item.id === 'send' ? activeCount : item.id === 'chat' ? unreadCount : 0
        return (
          <button
            key={item.id}
            data-testid={`nav-${item.id}`}
            className={`nav-item${active ? ' active' : ''}`}
            aria-current={active ? 'page' : undefined}
            title={item.label}
            onClick={() => setView(item.id)}
          >
            <Icon size={18} strokeWidth={2} style={{ flexShrink: 0 }} />
            <span className="nav-label">{item.label}</span>
            {badge > 0 && (
              <span className="count-badge" aria-label={item.id === 'send' ? `${badge} active` : `${badge} unread`}>
                {badge > 99 ? '99+' : badge}
              </span>
            )}
          </button>
        )
      })}

      {/* Opens the SuperFeedback panel (no floating button — it overlapped Send). */}
      <button className="nav-item sidebar-feedback" title="Send feedback" onClick={() => SuperFeedback.open()}>
        <MessageSquarePlus size={18} strokeWidth={2} style={{ flexShrink: 0 }} />
        <span className="nav-label">Feedback</span>
      </button>

      <div style={{ flex: 1 }} />

      <div className="sidebar-me" title={name || 'This device'}>
        <div className="sidebar-me-avatar">
          <FriendAvatar friend={{ name, avatar }} />
        </div>
        <div className="sidebar-me-text">
          <div className="truncate-1" style={{ fontSize: 'var(--font-sm)', fontWeight: 650 }}>
            {name || 'This device'}
          </div>
          <div style={{ fontSize: 'var(--font-xs)', color: 'var(--text-faint)' }}>This device</div>
        </div>
      </div>
    </nav>
  )
}
