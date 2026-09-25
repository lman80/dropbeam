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
import { avatarColor } from '../lib/avatar'
import { IconButton } from './ui'

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
      {/* macOS: clears the traffic lights and drags the window. */}
      <div className="sidebar-top titlebar-drag" data-tauri-drag-region />
      {NAV.map((item) => {
        const active = view === item.id
        const Icon = item.icon
        const count = item.id === 'send' ? activeCount : item.id === 'chat' ? unreadCount : 0
        return (
          <button
            key={item.id}
            data-testid={`nav-${item.id}`}
            className={`nav-item${active ? ' active' : ''}`}
            aria-current={active ? 'page' : undefined}
            title={item.label}
            onClick={() => setView(item.id)}
          >
            <Icon strokeWidth={1.75} />
            <span className="nav-label">{item.label}</span>
            {count > 0 && (
              <span
                className={`nav-count${item.id === 'chat' ? ' unread' : ' active-dot'}`}
                aria-label={item.id === 'send' ? `${count} in progress` : `${count} unread`}
              >
                {count > 99 ? '99+' : count}
              </span>
            )}
          </button>
        )
      })}

      <div className="sidebar-spacer" />

      <div className="sidebar-foot">
        <button
          className="sidebar-me"
          title="Your profile"
          onClick={() => setView('friends')}
        >
          <span className="sidebar-me-avatar" style={{ background: avatarColor(name || 'you') }}>
            <FriendAvatar friend={{ name: name || 'You', avatar }} />
          </span>
          <span className="sidebar-me-text">
            <span className="sidebar-me-name truncate-1" style={{ display: 'block' }}>{name || 'You'}</span>
          </span>
        </button>
        {/* Opens the SuperFeedback panel. Not a page, so it isn't a nav item; the
            click blurs it so no focus/selection state lingers after the panel closes. */}
        <IconButton
          label="Send feedback"
          className="sidebar-feedback"
          side="top"
          onClick={(e) => {
            e.currentTarget.blur()
            SuperFeedback.open()
          }}
        >
          <MessageSquarePlus strokeWidth={1.75} />
        </IconButton>
      </div>
    </nav>
  )
}
