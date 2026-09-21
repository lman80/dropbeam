import { useState } from 'react'
import { useStore } from '../store'
import { ChatView } from '../views/ChatView'
import { HistoryView } from '../views/HistoryView'
import { LocationsView } from '../views/LocationsView'
import { FriendsScreen, FriendDetail } from './FriendsScreen'
import { SendScreen } from './SendScreen'
import { SettingsScreen, type SettingsPage } from './SettingsScreen'

type Route = { kind: 'friend'; id: string } | { kind: 'settings'; page: SettingsPage }
/** App's content ErrorBoundary is keyed by top-level view; each tab gets a fresh local stack.
 * Native tab events remain owned by installNativeTabBar -> store.setView. */
export function MobileApp() {
  const view = useStore(s => s.view)
  const [stack, setStack] = useState<Route[]>([])
  const top = stack[stack.length - 1]
  const push = (route: Route) => setStack(previous => [...previous, route])
  const back = () => setStack(previous => previous.slice(0, -1))
  return <div className="mk-app">
    {view === 'send' && <SendScreen />}
    {view === 'friends' && (top?.kind === 'friend' ? <FriendDetail key={top.id} id={top.id} back={back} /> : <FriendsScreen push={id => push({ kind: 'friend', id })} />)}
    {view === 'settings' && <SettingsScreen key={top?.kind === 'settings' ? top.page : 'settings'} page={top?.kind === 'settings' ? top.page : 'settings'} push={page => push({ kind: 'settings', page })} back={back} />}
    {view === 'chat' && <div className="mk-legacy mk-legacy-chat"><ChatView /></div>}
    {view === 'history' && <div className="mk-legacy"><HistoryView /></div>}
    {(view === 'locations' || view === 'folders') && <div className="mk-legacy"><LocationsView /></div>}
  </div>
}
