/** Presentation only. No Tauri/store imports; clock is injectable for deterministic tests. */
export function presenceText(presence: { status: string; lastSeen: number | null }, now = Date.now()): string {
  if (presence.status === 'online') return 'Online now'
  if (presence.lastSeen == null || !Number.isFinite(presence.lastSeen)) return 'Offline'
  const minutes = Math.max(1, Math.floor((now - presence.lastSeen) / 60_000))
  if (minutes < 60) return `Last seen ${minutes} min ago`
  const hours = Math.floor(minutes / 60)
  if (hours < 24) return `Last seen ${hours} hr ago`
  const days = Math.floor(hours / 24)
  return `Last seen ${days} ${days === 1 ? 'day' : 'days'} ago`
}
/** Never guess whether an unrecognized code is a friend invite. */
export function friendCodeKind(value: string): 'invite' | 'permanent' | null {
  const code = value.trim()
  if (/^dropbeamf1:/i.test(code)) return 'invite'
  if (/^dropbeam:/i.test(code)) return 'permanent'
  return null
}
