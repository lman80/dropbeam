import { AnimatePresence, motion } from 'framer-motion'
import { ChevronRight, QrCode, Users, X } from 'lucide-react'
import { useStore } from '../store'
import { avatarGradient, initials } from '../lib/avatar'
import { friendOnlineState } from '../lib/presence'

function baseName(p: string): string {
  return p.split('/').pop() || p
}

export function SendToChooser() {
  const files = useStore((s) => s.pendingSend)
  const friends = useStore((s) => s.friends)
  const friendSeen = useStore((s) => s.friendSeen)
  const folderStatuses = useStore((s) => s.folderStatuses)
  const sendToFriend = useStore((s) => s.sendToFriend)
  const sendPaths = useStore((s) => s.sendPaths)
  const setPendingSend = useStore((s) => s.setPendingSend)
  const setView = useStore((s) => s.setView)

  const open = !!files && files.length > 0
  const close = () => setPendingSend(null)

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

  return (
    <AnimatePresence>
      {open && (
        <motion.div
          initial={{ opacity: 0 }}
          animate={{ opacity: 1 }}
          exit={{ opacity: 0 }}
          onClick={close}
          style={{
            position: 'fixed',
            inset: 0,
            background: 'rgba(8, 9, 14, 0.5)',
            backdropFilter: 'blur(4px)',
            display: 'grid',
            placeItems: 'center',
            zIndex: 200,
            padding: 20,
          }}
        >
          <motion.div
            initial={{ opacity: 0, scale: 0.96, y: 8 }}
            animate={{ opacity: 1, scale: 1, y: 0 }}
            exit={{ opacity: 0, scale: 0.97 }}
            transition={{ type: 'spring', stiffness: 320, damping: 28 }}
            onClick={(e) => e.stopPropagation()}
            className="card"
            style={{ width: 440, maxWidth: '100%', padding: 22, borderRadius: 20, maxHeight: '82vh', display: 'flex', flexDirection: 'column' }}
          >
            <div style={{ display: 'flex', justifyContent: 'space-between', alignItems: 'center', marginBottom: 4 }}>
              <div style={{ fontWeight: 750, fontSize: 16.5, minWidth: 0 }}>
                Send{' '}
                <span style={{ color: 'var(--accent)', whiteSpace: 'nowrap', overflow: 'hidden', textOverflow: 'ellipsis' }}>
                  {title}
                </span>{' '}
                to…
              </div>
              <button className="icon-btn" onClick={close}>
                <X size={17} />
              </button>
            </div>

            <div style={{ overflowY: 'auto', marginTop: 10, display: 'flex', flexDirection: 'column', gap: 6 }}>
              {friends.length === 0 ? (
                <button
                  className="btn btn-ghost"
                  style={{ width: '100%', justifyContent: 'flex-start', padding: '12px 14px' }}
                  onClick={() => {
                    close()
                    setView('friends')
                  }}
                >
                  <Users size={16} /> Add a friend to send by name
                </button>
              ) : (
                friends.map((f) => {
                  const online = friendOnlineState(f.name, friendSeen, folderStatuses)
                  return (
                    <button key={f.id} className="chooser-row" onClick={() => toFriend(f.id)}>
                      <span
                        style={{
                          position: 'relative',
                          width: 34,
                          height: 34,
                          borderRadius: 999,
                          display: 'grid',
                          placeItems: 'center',
                          color: 'white',
                          fontWeight: 700,
                          fontSize: 12,
                          flexShrink: 0,
                          background: avatarGradient(f.id),
                        }}
                      >
                        {initials(f.name)}
                        <span
                          title={online ? 'Online' : 'Status unknown'}
                          style={{
                            position: 'absolute',
                            right: -1,
                            bottom: -1,
                            width: 11,
                            height: 11,
                            borderRadius: 999,
                            background: online ? 'var(--green)' : 'var(--text-faint)',
                            border: '2.5px solid var(--surface)',
                          }}
                        />
                      </span>
                      <div style={{ flex: 1, minWidth: 0, textAlign: 'left' }}>
                        <div style={{ fontSize: 14, fontWeight: 650 }}>{f.name}</div>
                        <div style={{ fontSize: 11.5, color: online ? 'var(--green)' : 'var(--text-faint)' }}>
                          {online ? 'Online now' : 'Tap to send — they’ll get it when online'}
                        </div>
                      </div>
                      <ChevronRight size={17} style={{ color: 'var(--text-faint)', flexShrink: 0 }} />
                    </button>
                  )
                })
              )}
            </div>

            <div style={{ borderTop: '1px solid var(--border)', marginTop: 12, paddingTop: 12 }}>
              <button className="chooser-row" onClick={withCode}>
                <span
                  style={{
                    width: 34,
                    height: 34,
                    borderRadius: 11,
                    display: 'grid',
                    placeItems: 'center',
                    color: 'var(--accent)',
                    background: 'var(--accent-soft)',
                    flexShrink: 0,
                  }}
                >
                  <QrCode size={18} />
                </span>
                <div style={{ flex: 1, minWidth: 0, textAlign: 'left' }}>
                  <div style={{ fontSize: 14, fontWeight: 650 }}>Share with a code or QR</div>
                  <div style={{ fontSize: 11.5, color: 'var(--text-faint)' }}>
                    For anyone — they enter the code to receive
                  </div>
                </div>
                <ChevronRight size={17} style={{ color: 'var(--text-faint)', flexShrink: 0 }} />
              </button>
            </div>
          </motion.div>
        </motion.div>
      )}
    </AnimatePresence>
  )
}
