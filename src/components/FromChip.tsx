import { avatarGradient } from '../lib/avatar'
import { useStore } from '../store'
import { FriendAvatar } from './FriendAvatar'

/** "from Alex": who a received file came from, with their picture. The name is
 *  the sender as recorded when the file landed (the same provenance DropBeam
 *  stamps on the file itself); a friend with that name lends their photo. */
export function FromChip({ name }: { name: string }) {
  const friend = useStore(s => s.friends.find(f => f.name === name))
  const mine = useStore(s => s.myDevice?.account_pub)
  const own = !!mine && friend?.accountPub === mine
  return (
    <span className="from-chip" title={`Received from ${name}`}>
      <span className={`from-chip-avatar${own ? ' is-device' : ''}`} style={own ? undefined : { background: avatarGradient(friend?.id ?? name) }} aria-hidden>
        <FriendAvatar friend={friend ?? { name, avatar: null }} />
      </span>
      <span className="truncate-1">from {name}</span>
    </span>
  )
}
