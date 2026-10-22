import { HardDrive, RefreshCw } from 'lucide-react'
import type { Friend, SharedLocation } from '../../lib/api'
import { useStore } from '../../store'
import { friendPresence, presenceLabel } from '../../lib/presence'
import { Button, EmptyState, Row, Screen, Section } from '../kit'
import '../files.css'

export function LocationsScreen({ friends, shared, busy, hasErrors, refresh, open }: {
  friends: Friend[]; shared: Record<string, SharedLocation[]>; busy: boolean; hasErrors: boolean; refresh: () => Promise<void>; open: (friend: string, location: string) => void
}) {
  const myAccount = useStore(s => s.myDevice?.account_pub)
  const seen = useStore(s => s.friendSeen)
  const statuses = useStore(s => s.folderStatuses)
  const available = friends.filter(f => shared[f.id]?.length)
  const mine = available.filter(f => myAccount && f.accountPub === myAccount)
  const groups = mine.length ? [{ title: 'MY DEVICES', friends: mine }, { title: 'LOCATIONS', friends: available.filter(f => !mine.includes(f)) }] : [{ title: 'LOCATIONS', friends: available }]
  const unavailable = hasErrors || available.some(f => friendPresence(f.name, seen, statuses).status !== 'online')
  return <div className="files-screen"><Screen title="Locations" trailing={<Button aria-label="Refresh locations" disabled={busy} onClick={() => void refresh()}><RefreshCw size={22} /></Button>}>
    {groups.filter(g => g.friends.length).map(group => <Section key={group.title} title={group.title}>{group.friends.flatMap(friend => shared[friend.id].map(location => {
      const presence = friendPresence(friend.name, seen, statuses)
      return <Row key={`${friend.id}:${location.id}`} title={location.name} icon={<span className="files-tile files-tile-folder"><HardDrive size={21} /></span>} subtitle={<>{friend.name} · <i className={`files-presence ${presence.status}`} />{presence.status === 'online' ? 'Online now' : presenceLabel(presence)}</>} accessory="chevron" onPress={() => open(friend.id, location.id)} />
    }))}</Section>)}
    {!available.length && <EmptyState icon={<HardDrive />} title="No Locations" body="Folders your friends share with you appear here." />}
    {unavailable && <Section footer="Some devices are unavailable. Use the refresh button to try again." />}
  </Screen></div>
}
