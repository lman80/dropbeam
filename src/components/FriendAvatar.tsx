import { convertFileSrc } from '@tauri-apps/api/core'
import { HAS_TAURI, type Friend } from '../lib/api'
import { initials } from '../lib/avatar'

/** Shared by Chat and the send picker; local photos use the APPCONFIG asset scope. */
export function FriendAvatar({ friend }: { friend: Pick<Friend, 'name' | 'avatar'> }) {
  const avatar = typeof friend.avatar === 'string' ? friend.avatar : ''
  const src = avatar.startsWith('data:')
    ? avatar
    : avatar && HAS_TAURI ? convertFileSrc(avatar) : null
  return <>
    {initials(friend.name)}
    {src && <img key={src} className="avatar-img" src={src} alt=""
      onError={(e) => { e.currentTarget.style.display = 'none' }} />}
  </>
}
