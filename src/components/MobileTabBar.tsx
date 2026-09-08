import { History, MessageCircle, Send, Settings, Users } from 'lucide-react'
import type { LucideIcon } from 'lucide-react'
import { isActive } from '../lib/api'
import { useStore, type View } from '../store'

// The phone replacement for the desktop sidebar. "Shared Folders" is absent on
// purpose: it mirrors a folder on disk with a native folder picker and a file
// watcher, neither of which exists inside the iOS sandbox. "Feedback" is absent
// too — it screenshots the DOM into a GitHub issue, which is a desktop workflow.
const TABS: { id: View; label: string; icon: LucideIcon }[] = [
  { id: 'send', label: 'Send', icon: Send },
  { id: 'friends', label: 'Friends', icon: Users },
  { id: 'chat', label: 'Chat', icon: MessageCircle },
  { id: 'history', label: 'History', icon: History },
  { id: 'settings', label: 'Settings', icon: Settings },
]

export function MobileTabBar() {
  const view = useStore((s) => s.view)
  const setView = useStore((s) => s.setView)
  const activeCount = useStore(
    (s) => Object.values(s.transfers).filter((t) => isActive(t.state)).length,
  )
  const unreadCount = useStore((s) =>
    Object.values(s.chatUnread).reduce((a, b) => a + b, 0),
  )

  return (
    <nav className="tabbar" aria-label="Main">
      {TABS.map((tab) => {
        const active = view === tab.id
        const Icon = tab.icon
        const badge =
          tab.id === 'send' ? activeCount : tab.id === 'chat' ? unreadCount : 0
        return (
          <button
            key={tab.id}
            data-testid={`nav-${tab.id}`}
            className={`tabbar-item${active ? ' active' : ''}`}
            aria-current={active ? 'page' : undefined}
            onClick={() => setView(tab.id)}
          >
            <span className="tabbar-icon">
              <Icon size={22} strokeWidth={active ? 2.4 : 2} />
              {badge > 0 && <span className="tabbar-badge">{badge > 99 ? '99+' : badge}</span>}
            </span>
            <span className="tabbar-label">{tab.label}</span>
          </button>
        )
      })}
    </nav>
  )
}
