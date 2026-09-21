import { useRef, useState } from 'react'
import { HardDrive, MessageCircle, Plus, QrCode, Send, Users } from 'lucide-react'
import { QRCodeSVG } from 'qrcode.react'
import { api } from '../lib/api'
import { groupDevices } from '../lib/deviceIcons'
import { useStore } from '../store'
import { LinkNewDeviceModal } from '../components/LinkDeviceModal'
import { QrScanner } from '../components/QrScanner'
import { ActionSheet, Button, EmptyState, IconSquare, Row, Screen, SearchField, Section, Sheet, TextField } from './kit'
import { copyText, FriendAvatar, NameAlert, reportError, usePresence } from './shared'
import { friendCodeKind } from './helpers'

export function FriendsScreen({ push }: { push: (friendId: string) => void }) {
  const friends = useStore(s => s.friends)
  const myDevice = useStore(s => s.myDevice)
  const presence = usePresence()
  const [search, setSearch] = useState('')
  const [adding, setAdding] = useState(false)
  const [linking, setLinking] = useState(false)
  const all = groupDevices(friends, myDevice?.account_pub)
  const match = (name: string) => name.toLocaleLowerCase().includes(search.trim().toLocaleLowerCase())
  const row = (friend: typeof friends[number]) => <Row key={friend.id} avatar={<FriendAvatar friend={friend} />} title={friend.name} subtitle={presence(friend)} accessory="chevron" onPress={() => push(friend.id)} />
  return <Screen title="Friends" trailing={<Button aria-label="Add friend" onClick={() => setAdding(true)}><Plus size={24} /></Button>}>
    {friends.length > 6 && <SearchField value={search} onChange={setSearch} />}
    <Section title="My Devices" footer={!all.myDevices.length ? 'Link your phone or another computer to see all your devices here.' : undefined}>{all.myDevices.filter(f => match(f.name)).map(row)}<Row title="Link a Device" tint onPress={() => setLinking(true)} /></Section>
    <Section title="Friends" footer={search && !all.others.some(f => match(f.name)) ? 'No matching friends.' : undefined}>{all.others.filter(f => match(f.name)).map(row)}</Section>
    {!all.others.length && <EmptyState icon={<Users />} title="No Friends" body="Add a friend to send files and messages." />}
    {adding && <AddFriendSheet onClose={() => setAdding(false)} />}
    {linking && <LinkNewDeviceModal onClose={() => setLinking(false)} />}
  </Screen>
}
export function FriendDetail({ id, back }: { id: string; back: () => void }) {
  const friend = useStore(s => s.friends.find(f => f.id === id))
  const presence = usePresence()
  const [rename, setRename] = useState(false)
  const [remove, setRemove] = useState(false)
  const [sending, setSending] = useState(false)
  const [checking, setChecking] = useState(false)
  const [connection, setConnection] = useState('')
  const [invite, setInvite] = useState<string | null>(null)
  const [inviting, setInviting] = useState(false)
  if (!friend) return <Screen title="Friend" leading={{ title: 'Friends', onPress: back }}><EmptyState icon={<Users />} title="Friend Removed" body="This friend is no longer in your list." /></Screen>
  const send = async () => {
    if (sending) return
    setSending(true)
    try { const paths = await api.pickFiles(); if (paths.length) await useStore.getState().sendToFriend(friend.id, paths) }
    catch (error) { reportError(error) } finally { setSending(false) }
  }
  const showInvite = async () => {
    if (inviting) return
    setInviting(true)
    try { setInvite(await api.friendInvite(friend.id)) } catch (error) { reportError(error) } finally { setInviting(false) }
  }
  const check = async () => {
    if (checking) return
    setChecking(true); setConnection('Checking…')
    try {
      const state = useStore.getState()
      const online = await state.pingFriend(friend.id)
      const detail = online ? await state.probeFriend(friend.id) : null
      setConnection(!online ? 'No response' : detail ? `${detail.path === 'local' ? 'Local network' : detail.path === 'direct' ? 'Direct connection' : detail.path === 'relay' ? 'Connected through relay' : 'Connecting'}${detail.rttMs != null ? ` · ${Math.round(detail.rttMs)} ms` : ''}` : 'Online now')
    } catch (error) { setConnection('Could not check connection'); reportError(error) } finally { setChecking(false) }
  }
  return <Screen title={friend.name} leading={{ title: 'Friends', onPress: back }}>
    <div className="mk-profile"><FriendAvatar friend={friend} size={80} /><h2>{friend.name}</h2><p>{presence(friend)}</p></div>
    <Section><Row icon={<IconSquare><Send /></IconSquare>} title="Send Files" accessory="chevron" disabled={sending} onPress={() => void send()} /><Row icon={<IconSquare color="var(--mk-green)"><MessageCircle /></IconSquare>} title="Message" accessory="chevron" onPress={() => { void useStore.getState().openChat(id); useStore.getState().setView('chat') }} /><Row icon={<IconSquare color="var(--mk-gray)"><HardDrive /></IconSquare>} title="Browse Locations" accessory="chevron" onPress={() => useStore.getState().setView('locations')} /></Section>
    <Section title="Settings"><Row title="Accept files automatically" accessory="toggle" checked={friend.autoAccept} onChange={value => void useStore.getState().setFriendAutoAccept(id, value)} /><Row title="Name" value={friend.name} accessory="chevron" onPress={() => setRename(true)} /></Section>
    <Section><Row title="Check connection" subtitle={connection || undefined} tint disabled={checking} onPress={() => void check()} /><Row title="Share Invite" tint disabled={inviting} onPress={() => void showInvite()} /></Section>
    <Section><Row title="Remove Friend" destructive onPress={() => setRemove(true)} /></Section>
    {invite && <Sheet title="Friend Invite" onClose={() => setInvite(null)}><Section footer={`Share this invite with ${friend.name}.`}><div className="mk-qr"><QRCodeSVG value={invite} size={240} level="M" /></div><Row title="Copy Invite" tint onPress={() => void copyText(invite)} /></Section></Sheet>}
    {rename && <NameAlert initial={friend.name} onSave={name => useStore.getState().renameFriend(id, name)} onClose={() => setRename(false)} />}
    {remove && <ActionSheet title="Remove Friend" message={`Remove ${friend.name}? Chat history stays on this device.`} onClose={() => setRemove(false)} actions={[{ label: 'Remove Friend', destructive: true, onPress: () => void useStore.getState().removeFriend(id).then(back).catch(reportError) }]} />}
  </Screen>
}
function AddFriendSheet({ onClose }: { onClose: () => void }) {
  const [code, setCode] = useState('')
  const [scan, setScan] = useState(false)
  const [busy, setBusy] = useState(false)
  const lock = useRef(false)
  const [error, setError] = useState('')
  const submit = async (value = code) => {
    if (lock.current) return
    const kind = friendCodeKind(value)
    if (!kind) { setError("That doesn’t look like a DropBeam code."); return }
    lock.current = true; setBusy(true); setError('')
    try {
      const state = useStore.getState()
      if (kind === 'invite') await state.acceptFriend(value.trim())
      else await state.addFriendByCode(value.trim())
      onClose()
    } catch (e) { setError(String(e)) } finally { lock.current = false; setBusy(false) }
  }
  // Keep the parent sheet mounted so opening/closing the scanner preserves its focus and code.
  return <><Sheet title="Add Friend" onClose={onClose} primary={<Button disabled={busy || !code.trim()} onClick={() => void submit()}>{busy ? 'Adding…' : 'Done'}</Button>}><form onSubmit={e => { e.preventDefault(); void submit() }}><Section title="Their Code" footer="Ask your friend for their DropBeam code, or scan it from their screen."><TextField label="Their code" placeholder="Paste code" autoFocus value={code} onChange={e => setCode(e.target.value)} /><Row icon={<IconSquare><QrCode /></IconSquare>} title="Scan QR Code" accessory="chevron" disabled={busy} onPress={() => setScan(true)} /></Section></form>{error && <Section footer={<span className="mk-error" role="alert">{error}</span>} />}</Sheet>{scan && <QrScanner hint="Scan your friend’s DropBeam code." onClose={() => setScan(false)} onResult={value => { setScan(false); setCode(value); void submit(value) }} />}</>
}
