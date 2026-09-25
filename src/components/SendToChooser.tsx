import { Sheet, Section, Row, IconSquare } from '../mobile/kit'
import { folderName } from '../lib/syncedFolders'
import { FriendAvatar as MobileAvatar } from '../mobile/shared'
import { presenceText } from '../mobile/helpers'
import { friendPresence } from '../lib/presence'
import { Fragment } from 'react'
import { groupDevices } from '../lib/deviceIcons'
import { useOwnDeviceLabels } from '../lib/ownDevices'
import { MOBILE_UI } from '../lib/platform'
import { useEffect } from 'react'
import { AnimatePresence } from 'framer-motion'
import { QrCode, UserPlus } from 'lucide-react'
import { Dialog } from './Dialog'
import { useStore } from '../store'
import { avatarGradient } from '../lib/avatar'
import { FriendAvatar } from './FriendAvatar'
import { claimPresenceChecks, presenceLabel } from '../lib/presence'

function baseName(p: string): string {
  return folderName(p) // splits on / and \ (Windows paths)
}

export function SendToChooser() {
  const files = useStore((s) => s.pendingSend)
  const friends = useStore((s) => s.friends)
  const myDevice = useStore(s => s.myDevice)
  const { myDevices, others } = groupDevices(friends, myDevice?.account_pub)
  const friendSeen = useStore((s) => s.friendSeen)
  const folderStatuses = useStore((s) => s.folderStatuses)
  const sendToFriend = useStore((s) => s.sendToFriend)
  const sendPaths = useStore((s) => s.sendPaths)
  const setPendingSend = useStore((s) => s.setPendingSend)
  const setView = useStore((s) => s.setView)
  const ownLabels = useOwnDeviceLabels()

  const open = !!files && files.length > 0
  const close = () => setPendingSend(null)

  // #34: the sheet is where a stale "offline" hurts most — it's the moment you
  // pick who to send to. Opening it actively re-checks everyone who doesn't
  // already read as online (rate-limited inside claimPresenceChecks).
  useEffect(() => {
    if (!open) return
    void useStore.getState().refreshMyDevice().catch(() => {})
    const s = useStore.getState()
    for (const id of claimPresenceChecks(s.friends, s.friendSeen, s.folderStatuses)) void s.pingFriend(id)
  }, [open])

  const title =
    files && files.length === 1
      ? baseName(files[0])
      : `${files?.length ?? 0} files`

  const toFriend = (id: string) => {
    if (files) sendToFriend(id, files)
    close()
  }
  const withCode = () => {
    if (files) sendPaths(files)
    close()
  }

  if (MOBILE_UI) return open ? <Sheet title="Send to" onClose={close}>
    {[{ title: 'My Devices', items: myDevices }, { title: 'Friends', items: others }].map(group => <Section key={group.title} title={group.title} footer={!group.items.length ? (group.title === 'My Devices' ? 'Your linked devices appear here.' : 'Add a friend to send by name.') : undefined}>{group.items.map(friend => <Row key={friend.id} avatar={<MobileAvatar friend={friend} />} title={friend.name} subtitle={presenceText(friendPresence(friend.name, friendSeen, folderStatuses))} onPress={() => toFriend(friend.id)} />)}</Section>)}
    <Section title="Or"><Row icon={<IconSquare><QrCode /></IconSquare>} title="Quick Send (code)" accessory="chevron" onPress={withCode} /></Section>
  </Sheet> : null

  const people = [{ title: 'My devices', items: myDevices }, { title: 'Friends', items: others }].filter((g) => g.items.length)
  const anyOffline = friends.some((f) => friendPresence(f.name, friendSeen, folderStatuses).status !== 'online')

  return (
    <AnimatePresence>
      {open && (
        <Dialog
          title="Send to"
          subtitle={<span className="truncate-1" style={{ display: 'block' }} title={files?.join('\n')}>{title}</span>}
          onClose={close}
          width={400}
          className="chooser-dialog"
          footer={
            <button className="chooser-row chooser-share" onClick={withCode}>
              <span className="chooser-avatar chooser-glyph" aria-hidden><QrCode size={16} /></span>
              <span className="chooser-text">
                <span className="chooser-name">Share with a code or QR code</span>
                <span className="chooser-sub">For anyone, even without DropBeam friends</span>
              </span>
            </button>
          }
        >
          {friends.length === 0 ? (
            <button
              className="chooser-row"
              onClick={() => {
                close()
                setView('friends')
              }}
            >
              <span className="chooser-avatar chooser-glyph" aria-hidden><UserPlus size={16} /></span>
              <span className="chooser-text">
                <span className="chooser-name">Add a friend</span>
                <span className="chooser-sub">Then send to them by name</span>
              </span>
            </button>
          ) : (
            <>
              {people.map((group) => (
                <Fragment key={group.title}>
                  <h3 className="chooser-section">{group.title}</h3>
                  {group.items.map((f) => {
                    const presence = friendPresence(f.name, friendSeen, folderStatuses)
                    const online = presence.status === 'online'
                    const name = ownLabels[f.id] ?? f.name
                    const own = !!ownLabels[f.id]
                    return (
                      <button key={f.id} className="chooser-row" onClick={() => toFriend(f.id)} title={`Send to ${name}`}>
                        <span className={`chooser-avatar${own ? ' chooser-glyph' : ''}`} style={own ? undefined : { background: avatarGradient(f.id) }} aria-hidden>
                          <FriendAvatar friend={f} />
                          {online && <span className="presence-dot online" />}
                        </span>
                        <span className="chooser-text">
                          <span className="chooser-name truncate-1">{name}</span>
                          <span className="chooser-sub truncate-1">{online ? 'Online' : presenceLabel(presence)}</span>
                        </span>
                        <span className="chooser-go" aria-hidden>Send</span>
                      </button>
                    )
                  })}
                </Fragment>
              ))}
              {/* Honest, said once: friend file sends retry ~90s then fail — there is
                  no store-and-forward for files (chat messages DO queue). */}
              {anyOffline && <p className="chooser-note">If someone’s offline, DropBeam keeps trying for about 2 minutes.</p>}
            </>
          )}
        </Dialog>
      )}
    </AnimatePresence>
  )
}
