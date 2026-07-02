import { useMemo, useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import { ArrowDownToLine, Inbox } from 'lucide-react'
import { api } from '../lib/api'
import { useStore } from '../store'
import { DropZone } from '../components/DropZone'
import { TransferCard } from '../components/TransferCard'
import { EmptyState } from '../components/bits'

export function SendView() {
  const transfers = useStore((s) => s.transfers)
  const order = useStore((s) => s.order)
  const dragHovering = useStore((s) => s.dragHovering)
  const setPendingSend = useStore((s) => s.setPendingSend)
  const receiveCode = useStore((s) => s.receiveCode)
  const [code, setCode] = useState('')
  const [showReceive, setShowReceive] = useState(false)

  // One unified, newest-first list of everything — sends AND receives. Ghost
  // entries (a discarded marker like a ping) are filtered out.
  const list = useMemo(
    () =>
      order
        .map((id) => transfers[id])
        .filter(Boolean)
        .reverse()
        .filter((t) => !(t.state === 'canceled' && t.fileNames.length === 0)),
    [order, transfers],
  )

  const onPick = async () => {
    const paths = await api.pickFiles()
    if (paths.length) setPendingSend(paths)
  }

  const submitReceive = async (e: React.FormEvent) => {
    e.preventDefault()
    if (!code.trim()) return
    // Only clear the field + close the panel if the receive actually started —
    // otherwise the user loses what they typed before they can read the error.
    const ok = await receiveCode(code)
    if (ok) {
      setCode('')
      setShowReceive(false)
    }
  }

  return (
    <div style={{ maxWidth: 660, margin: '0 auto', padding: '8px 28px 36px' }}>
      <DropZone hovering={dragHovering} onPick={onPick} />

      {/* Receiving by code is secondary now — friend transfers arrive on their own. */}
      <div style={{ marginTop: 12, display: 'flex', justifyContent: 'center' }}>
        {!showReceive ? (
          <button
            className="btn btn-ghost"
            style={{ fontSize: 12.5 }}
            onClick={() => setShowReceive(true)}
          >
            <ArrowDownToLine size={14} /> Have a code? Receive files
          </button>
        ) : (
          <form onSubmit={submitReceive} style={{ display: 'flex', gap: 8, width: '100%', maxWidth: 440 }}>
            <input
              className="input"
              placeholder="Paste the code the sender shared…"
              value={code}
              autoFocus
              spellCheck={false}
              autoCapitalize="off"
              autoCorrect="off"
              onChange={(e) => setCode(e.target.value)}
              style={{ fontFamily: 'var(--font-mono)', fontSize: 14 }}
            />
            <button className="btn btn-primary" type="submit" disabled={!code.trim()}>
              <ArrowDownToLine size={15} /> Receive
            </button>
            <button
              className="btn btn-ghost"
              type="button"
              onClick={() => {
                setShowReceive(false)
                setCode('')
              }}
            >
              Cancel
            </button>
          </form>
        )}
      </div>

      <div style={{ marginTop: 22, display: 'flex', flexDirection: 'column', gap: 12 }}>
        <AnimatePresence initial={false}>
          {list.map((t) => (
            <TransferCard key={t.id} t={t} />
          ))}
        </AnimatePresence>

        {list.length === 0 && (
          <EmptyState
            icon={<Inbox size={24} />}
            title="Nothing here yet"
            hint="Drag files onto the area above and pick who to send to — a friend, or anyone with a code. Whatever you receive shows up here automatically, too. Tip: you can also drop a file straight onto the DropBeam menu-bar icon to send it to a friend."
          />
        )}
      </div>
    </div>
  )
}
