import { fileSrc, HAS_TAURI, type Friend } from '../lib/api'
import { initials } from '../lib/avatar'
import { deviceIcon } from '../lib/deviceIcons'
import { useStore } from '../store'

/** Shared by Chat and the send picker; local photos use the APPCONFIG asset scope.
 *  One of the user's own devices shows its device glyph instead of a photo. */
export function FriendAvatar({ friend }: { friend: Pick<Friend, 'name' | 'avatar'> & Partial<Pick<Friend, 'accountPub' | 'deviceKind' | 'deviceOs'>> }) {
  const account = useStore(s => s.myDevice?.account_pub)
  if (account && friend.accountPub === account) {
    const Icon = deviceIcon(friend.deviceOs === 'macos' && friend.deviceKind !== 'desktop' ? 'laptop' : friend.deviceKind ?? undefined)
    return <Icon className="avatar-device" size={18} strokeWidth={1.8} aria-hidden />
  }
  const avatar = typeof friend.avatar === 'string' ? friend.avatar : ''
  const src = avatar.startsWith('data:')
    ? avatar
    : avatar && HAS_TAURI ? fileSrc(avatar) : null
  return <>
    {initials(friend.name)}
    {src && <img key={src} className="avatar-img" src={src} alt=""
      onError={(e) => { e.currentTarget.style.display = 'none' }} />}
  </>
}
