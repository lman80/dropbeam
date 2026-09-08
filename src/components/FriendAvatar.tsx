import { convertFileSrc } from '@tauri-apps/api/core'
import { HAS_TAURI, type Friend } from '../lib/api'
import { initials } from '../lib/avatar'

/** Shared by Chat and the send picker; local photos use the APPCONFIG asset scope. */
export function FriendAvatar({ friend }: { friend: Friend }) {
  const src = friend.avatar?.startsWith('data:')
    ? friend.avatar
    : friend.avatar && HAS_TAURI ? convertFileSrc(friend.avatar) : null
  return <>
    {initials(friend.name)}
    {src && <img key={src} className="avatar-img" src={src} alt=""
      onError={(e) => { e.currentTarget.style.display = 'none' }} />}
  </>
}
