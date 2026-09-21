import { useEffect, useState } from 'react'
import { fileSrc, type Friend } from '../lib/api'
import { deviceIcon } from '../lib/deviceIcons'
import { friendPresence, claimPresenceChecks } from '../lib/presence'
import { useStore } from '../store'
import { Alert, Avatar, TextField } from './kit'
import { presenceText } from './helpers'

export function reportError(error: unknown) { useStore.getState().toast('error', String(error)) }
export async function copyText(text: string) {
  try { await navigator.clipboard.writeText(text); useStore.getState().toast('success', 'Code copied') }
  catch (error) { reportError(error) }
}
export function FriendAvatar({ friend, size = 40 }: { friend: Friend; size?: number }) {
  const Device = deviceIcon(friend.deviceKind || undefined)
  return <Avatar name={friend.name} src={friend.avatar ? fileSrc(friend.avatar) : undefined} size={size} badge={<Device />} />
}
export function usePresence() {
  const seen = useStore(s => s.friendSeen)
  const statuses = useStore(s => s.folderStatuses)
  const [now, setNow] = useState(Date.now)
  useEffect(() => {
    const state = useStore.getState()
    void state.refreshMyDevice().catch(reportError)
    for (const id of claimPresenceChecks(state.friends, state.friendSeen, state.folderStatuses)) void state.pingFriend(id).catch(reportError)
    const timer = setInterval(() => setNow(Date.now()), 30_000)
    return () => clearInterval(timer)
  }, [])
  return (friend: Friend) => presenceText(friendPresence(friend.name, seen, statuses), now)
}
export function NameAlert({ title = 'Name', initial, onSave, onClose }: { title?: string; initial: string; onSave: (name: string) => Promise<void>; onClose: () => void }) {
  const [name, setName] = useState(initial)
  const [busy, setBusy] = useState(false)
  const save = async () => { if (busy || !name.trim()) return; setBusy(true); try { await onSave(name.trim()); onClose() } catch (error) { reportError(error) } finally { setBusy(false) } }
  return <Alert title={title} onClose={onClose} actions={[{ label: 'Cancel', onPress: onClose }, { label: 'Save', disabled: !name.trim() || busy, onPress: () => void save() }]}><form onSubmit={e => { e.preventDefault(); void save() }}><TextField label="Name" autoFocus autoCapitalize="words" maxLength={40} value={name} onChange={e => setName(e.target.value)} /></form></Alert>
}
