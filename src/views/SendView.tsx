import { useMemo, useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import { ArrowDownToLine, Inbox } from 'lucide-react'
import { api } from '../lib/api'
import { useStore } from '../store'
import { MOBILE_UI } from '../lib/platform'
import { MobileHeader } from '../components/MobileHeader'
import { DropZone } from '../components/DropZone'
import { TransferCard } from '../components/TransferCard'
import { EmptyState } from '../components/bits'

export function SendView() {
  const transfers = useStore((s) => s.transfers)
  const order = useStore((s) => s.order)
  const dragHovering = useStore((s) => s.dragHovering)
  const setPendingSend = useStore((s) => s.setPendingSend)
  const receiveCode = useStore((s) => s.receiveCode)
  const [picking, setPicking] = useState(false)
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

  const onPick = async (source: 'files' | 'photos' | 'folder' = 'files') => {
    if (picking) return
    setPicking(true)
    try {
      const paths = source === 'photos'
        ? await api.pickPhotos()
        : source === 'folder'
          ? [await api.pickDirectory()].filter((p): p is string => p !== null)
          : await api.pickFiles()
      if (paths.length) setPendingSend(paths)
    } catch (error) {
      useStore.getState().toast('error', String(error))
    } finally {
      setPicking(false)
      useStore.getState().setDragHovering(false)
    }
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

  if (MOBILE_UI) return (
    <div className="mobile-page mobile-send">
      <MobileHeader title="Send" />
      <div className="mobile-inset mobile-stack">
        <DropZone hovering={dragHovering} picking={picking} onPick={() => void onPick()} onPickPhotos={() => void onPick('photos')} />
        {!showReceive ? <button className="ios-button glass glass-pill" onClick={() => setShowReceive(true)}><ArrowDownToLine size={18} />Have a code? Receive files</button> :
          <div className="dialog-overlay"><form role="dialog" aria-modal="true" aria-label="Receive files" className="mobile-receive glass dialog" onSubmit={submitReceive} onKeyDown={e => {
            if (e.key === 'Escape') setShowReceive(false)
            if (e.key === 'Tab') {
              const controls = Array.from(e.currentTarget.querySelectorAll<HTMLElement>('input, button:not(:disabled)'))
              const first = controls[0], last = controls[controls.length - 1]
              if (e.shiftKey && document.activeElement === first) { e.preventDefault(); last?.focus() }
              if (!e.shiftKey && document.activeElement === last) { e.preventDefault(); first?.focus() }
            }
          }}><h2 className="ios-title2">Receive files</h2>
            <input className="input" aria-label="Receive code" placeholder="Paste receive code" autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" value={code} autoFocus onChange={e => setCode(e.target.value)} />
            <div className="mobile-equal"><button className="ios-button ios-primary" disabled={!code.trim()}>Receive</button><button className="ios-button" type="button" onClick={() => { setShowReceive(false); setCode('') }}>Cancel</button></div>
          </form></div>}
      </div>
      <div className="mobile-inset mobile-transfers mobile-stack">
        <AnimatePresence initial={false}>{list.map(t => <TransferCard key={t.id} t={t} />)}</AnimatePresence>
        {list.length === 0 && <div className="mobile-empty"><Inbox size={28} /><h2 className="ios-headline">Nothing here yet</h2><p className="ios-sub">Send photos, videos or documents.</p><button className="ios-button ios-primary" onClick={() => void onPick()}>Send files</button></div>}
      </div>
    </div>
  )

  return (
    <div
      style={{
        maxWidth: 660,
        margin: '0 auto',
        padding: MOBILE_UI ? '4px 16px 24px' : '8px 28px 36px',
      }}
    >
      <DropZone hovering={dragHovering} onPick={() => void onPick()} onPickPhotos={() => void onPick('photos')} picking={picking} />

      {/* Receiving by code is secondary now — friend transfers arrive on their own. */}
      <div style={{ marginTop: 12, display: 'flex', justifyContent: 'center', alignItems: 'center', gap: 8, flexWrap: 'wrap' }}>
        {!MOBILE_UI && !/Mac/i.test(navigator.userAgent) && (
          <button className="btn btn-ghost" disabled={picking} onClick={() => void onPick('folder')} title="On Linux and Windows, folders use a separate picker. Send a folder and all its contents.">
            Choose a folder
          </button>
        )}
        {!showReceive ? (
          <button
            className="btn btn-ghost"
            style={{ fontSize: 'calc(12.5px * var(--ui-font-scale, 1))' }}
            onClick={() => setShowReceive(true)}
          >
            <ArrowDownToLine size={14} /> Have a code? Receive files
          </button>
        ) : (
          <form onSubmit={submitReceive} style={{ display: 'flex', gap: 8, width: '100%', maxWidth: 440 }}>
            <input
              className="input"
              autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" inputMode="text"
              placeholder="Paste the code the sender shared…"
              value={code}
              autoFocus
              onChange={(e) => setCode(e.target.value)}
              style={{ fontFamily: 'var(--font-mono)', fontSize: 'calc(14px * var(--ui-font-scale, 1))' }}
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
            hint={
              MOBILE_UI
                ? 'Tap Photos or Files above, then pick who to send them to — a friend, or anyone with a code. Anything sent to you shows up here automatically.'
                : 'Drag files onto the area above and pick who to send to — a friend, or anyone with a code. Whatever you receive shows up here automatically, too. Tip: you can also drop a file straight onto the DropBeam menu-bar icon to send it to a friend.'
            }
          />
        )}
      </div>
    </div>
  )
}
