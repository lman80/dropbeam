import { Sheet, Section, Row, IconSquare } from '../mobile/kit'
import { folderName } from '../lib/syncedFolders'
import { FriendAvatar as MobileAvatar } from '../mobile/shared'
import { presenceText } from '../mobile/helpers'
import { friendPresence } from '../lib/presence'
import { Fragment } from 'react'
import { groupDevices } from '../lib/deviceIcons'
import { DeviceBadge } from './DeviceBadge'
import { MOBILE_UI } from '../lib/platform'
import { useEffect } from 'react'
import { AnimatePresence } from 'framer-motion'
import { ChevronRight, QrCode, Send, Users } from 'lucide-react'
import { Dialog } from './Dialog'
import { useStore } from '../store'
import { avatarGradient } from '../lib/avatar'
import { FriendAvatar } from './FriendAvatar'
import { claimPresenceChecks, friendOnlineState } from '../lib/presence'

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

  return (
    <AnimatePresence>
      {open && (
        <Dialog
          title="Send to…"
          subtitle={<span className="truncate-1" style={{ display: 'block' }} title={files?.join('\n')}>{title}</span>}
          icon={<Send size={17} />}
          onClose={close}
          bodyStyle={{ display: 'flex', flexDirection: 'column', gap: 2, margin: '0 -8px', padding: '0 8px' }}
          footer={
            <button className="chooser-row" onClick={withCode} style={{ margin: '0 -8px' }}>
              <span className="chooser-icon"><QrCode size={18} /></span>
              <div style={{ flex: 1, minWidth: 0, textAlign: 'left' }}>
                <div className="chooser-name">Share with a code or QR</div>
                <div className="chooser-sub">For anyone — they scan the QR or paste the code</div>
              </div>
              <ChevronRight size={17} style={{ color: 'var(--text-faint)', flexShrink: 0 }} />
            </button>
          }
        >
          {friends.length === 0 ? (
            <button
              className="card empty-row"
              onClick={() => {
                close()
                setView('friends')
              }}
            >
              <span className="empty-row-icon"><Users size={16} /></span>
              <span style={{ flex: 1, minWidth: 0 }}>
                <span className="empty-row-title">Add a friend to send by name</span>
                <span className="empty-row-sub">Friends get files in one click — no codes.</span>
              </span>
              <ChevronRight size={16} style={{ color: 'var(--text-faint)', flexShrink: 0 }} />
            </button>
          ) : (
            [ { title: 'My devices', items: myDevices }, { title: 'Friends', items: others } ].filter(group => group.items.length).map((group, gi) => <Fragment key={group.title}><h3 className="section-title" style={{ margin: gi ? '12px 8px 4px' : '2px 8px 4px' }}>{group.title}</h3>{group.items.map((f) => {
              const online = friendOnlineState(f.name, friendSeen, folderStatuses)
              return (
                <button key={f.id} className="chooser-row" onClick={() => toFriend(f.id)}>
                  <span className="chooser-avatar" style={{ background: avatarGradient(f.id) }}>
                    <FriendAvatar friend={f} /><DeviceBadge kind={f.deviceKind} />
                    <span className={`presence-dot${online ? ' online' : ''}`} title={online ? 'Online' : 'Status unknown'} />
                  </span>
                  <div style={{ flex: 1, minWidth: 0, textAlign: 'left' }}>
                    <div className="chooser-name truncate-1">{f.name}</div>
                    <div className="chooser-sub" style={{ color: online ? 'var(--green)' : undefined }}>
                      {/* Honest: friend file sends retry ~90s then fail — there is
                          no store-and-forward for files (chat messages DO queue). */}
                      {online ? 'Online now' : 'Offline — a send keeps trying for ~2 minutes'}
                    </div>
                  </div>
                  <ChevronRight size={17} style={{ color: 'var(--text-faint)', flexShrink: 0 }} />
                </button>
              )
            })}</Fragment>)
          )}
        </Dialog>
      )}
    </AnimatePresence>
  )
}
