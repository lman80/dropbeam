import { useMemo, useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { ArrowDownToLine, Inbox, Send, UserPlus } from 'lucide-react'
import { api, isActive } from '../lib/api'
import { useStore } from '../store'
import { DropZone } from '../components/DropZone'
import { TransferCard } from '../components/TransferCard'
import { EmptyState, Spinner } from '../components/bits'
import { avatarGradient, initials } from '../lib/avatar'

export function SendView() {
  const [mode, setMode] = useState<'send' | 'receive'>('send')
  const [code, setCode] = useState('')
  const [pickingFor, setPickingFor] = useState<string | null>(null)
  const transfers = useStore((s) => s.transfers)
  const order = useStore((s) => s.order)
  const dragHovering = useStore((s) => s.dragHovering)
  const sendPaths = useStore((s) => s.sendPaths)
  const receiveCode = useStore((s) => s.receiveCode)
  const friends = useStore((s) => s.friends)
  const sendToFriend = useStore((s) => s.sendToFriend)
  const setView = useStore((s) => s.setView)

  const list = useMemo(
    () => order.map((id) => transfers[id]).filter(Boolean).reverse(),
    [order, transfers],
  )
  const sends = list.filter((t) => t.direction === 'send')
  const receives = list.filter((t) => t.direction === 'receive')
  const shown = mode === 'send' ? sends : receives
  const activeSends = sends.filter((t) => isActive(t.state)).length
  const activeReceives = receives.filter((t) => isActive(t.state)).length

  const onPick = async () => {
    const paths = await api.pickFiles()
    if (paths.length) sendPaths(paths)
  }

  const beamToFriend = async (id: string) => {
    setPickingFor(id)
    try {
      const paths = await api.pickFiles()
      if (paths.length) await sendToFriend(id, paths)
    } finally {
      setPickingFor(null)
    }
  }

  const submitReceive = (e: React.FormEvent) => {
    e.preventDefault()
    if (!code.trim()) return
    receiveCode(code)
    setCode('')
  }

  return (
    <div style={{ maxWidth: 660, margin: '0 auto', padding: '8px 28px 36px' }}>
      <div
        style={{
          display: 'flex',
          justifyContent: 'center',
          marginBottom: 22,
        }}
      >
        <div className="seg">
          <button className={mode === 'send' ? 'active' : ''} onClick={() => setMode('send')}>
            <Send size={15} /> Send
            {activeSends > 0 && <SegBadge n={activeSends} />}
          </button>
          <button className={mode === 'receive' ? 'active' : ''} onClick={() => setMode('receive')}>
            <ArrowDownToLine size={15} /> Receive
            {activeReceives > 0 && <SegBadge n={activeReceives} />}
          </button>
        </div>
      </div>

      <AnimatePresence mode="wait">
        <motion.div
          key={mode}
          initial={{ opacity: 0, y: 8 }}
          animate={{ opacity: 1, y: 0 }}
          exit={{ opacity: 0, y: -8 }}
          transition={{ duration: 0.18 }}
        >
          {mode === 'send' ? (
            <DropZone hovering={dragHovering} onPick={onPick} />
          ) : (
            <form onSubmit={submitReceive} className="card" style={{ padding: 20 }}>
              <div style={{ fontSize: 15, fontWeight: 700, marginBottom: 4 }}>Receive files</div>
              <div style={{ fontSize: 13, color: 'var(--text-muted)', marginBottom: 14 }}>
                Enter the code shown on the sending device.
              </div>
              <div style={{ display: 'flex', gap: 10 }}>
                <input
                  className="input"
                  placeholder="e.g. 8425-mizar-cobalt"
                  value={code}
                  autoFocus
                  spellCheck={false}
                  autoCapitalize="off"
                  autoCorrect="off"
                  onChange={(e) => setCode(e.target.value)}
                  style={{ fontFamily: 'var(--font-mono)', fontSize: 15 }}
                />
                <button className="btn btn-primary" type="submit" disabled={!code.trim()}>
                  <ArrowDownToLine size={16} /> Receive
                </button>
              </div>
            </form>
          )}
        </motion.div>
      </AnimatePresence>

      {mode === 'send' && (
        <div style={{ marginTop: 18 }}>
          <div
            style={{
              display: 'flex',
              alignItems: 'center',
              gap: 8,
              marginBottom: 10,
            }}
          >
            <div style={{ flex: 1, height: 1, background: 'var(--border)' }} />
            <span style={{ fontSize: 11.5, color: 'var(--text-faint)', fontWeight: 600 }}>
              {friends.length ? 'OR BEAM TO A FRIEND' : 'FRIENDS'}
            </span>
            <div style={{ flex: 1, height: 1, background: 'var(--border)' }} />
          </div>

          {friends.length ? (
            <div style={{ display: 'flex', gap: 9, flexWrap: 'wrap', justifyContent: 'center' }}>
              {friends.map((f) => (
                <button
                  key={f.id}
                  className="card"
                  onClick={() => beamToFriend(f.id)}
                  disabled={pickingFor !== null}
                  title={`Send files to ${f.name}`}
                  style={{
                    display: 'flex',
                    alignItems: 'center',
                    gap: 9,
                    padding: '7px 13px 7px 8px',
                    borderRadius: 999,
                    cursor: 'pointer',
                    opacity: pickingFor && pickingFor !== f.id ? 0.5 : 1,
                  }}
                >
                  <span
                    style={{
                      width: 28,
                      height: 28,
                      borderRadius: 999,
                      display: 'grid',
                      placeItems: 'center',
                      color: 'white',
                      fontWeight: 700,
                      fontSize: 11,
                      background: avatarGradient(f.id),
                      flexShrink: 0,
                    }}
                  >
                    {pickingFor === f.id ? <Spinner size={13} /> : initials(f.name)}
                  </span>
                  <span style={{ fontSize: 13.5, fontWeight: 600 }}>{f.name}</span>
                </button>
              ))}
            </div>
          ) : (
            <div style={{ textAlign: 'center' }}>
              <button
                className="btn btn-ghost"
                onClick={() => setView('friends')}
                style={{ fontSize: 12.5 }}
              >
                <UserPlus size={14} /> Add a friend to send without codes
              </button>
            </div>
          )}
        </div>
      )}

      <div style={{ marginTop: 24, display: 'flex', flexDirection: 'column', gap: 12 }}>
        <AnimatePresence initial={false}>
          {shown.map((t) => (
            <TransferCard key={t.id} t={t} />
          ))}
        </AnimatePresence>

        {shown.length === 0 && (
          <EmptyState
            icon={mode === 'send' ? <Send size={24} /> : <Inbox size={24} />}
            title={mode === 'send' ? 'No active sends' : 'No active receives'}
            hint={
              mode === 'send'
                ? 'Drag in a file or click the area above. You’ll get a code to share — works on your network or anywhere over the internet.'
                : 'Paste a code from someone sending you files — or just wait. Files from friends arrive here automatically and land in your downloads.'
            }
          />
        )}
      </div>

      {shown.some((t) => isActive(t.state)) && (
        <div style={{ height: 8 }} />
      )}
    </div>
  )
}

function SegBadge({ n }: { n: number }) {
  return (
    <span
      className="chip"
      style={{
        background: 'var(--accent)',
        color: 'white',
        minWidth: 18,
        justifyContent: 'center',
        padding: '1px 5px',
        marginLeft: 6,
        fontSize: 11,
      }}
    >
      {n}
    </span>
  )
}
