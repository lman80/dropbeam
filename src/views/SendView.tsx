import { useMemo, useState } from 'react'
import { AnimatePresence } from 'framer-motion'
import { ArrowDownToLine, FolderUp, Inbox, Send as SendIcon } from 'lucide-react'
import { api, isActive } from '../lib/api'
import { useStore } from '../store'
import { IS_MAC, MOBILE_UI } from '../lib/platform'
import { MobileHeader } from '../components/MobileHeader'
import { DropZone } from '../components/DropZone'
import { TransferCard } from '../components/TransferCard'
import { ScanCodeButton } from '../components/CodeQr'
import { Dialog } from '../components/Dialog'
import { SectionHeader } from '../components/ui'

export function SendView() {
  const transfers = useStore((s) => s.transfers)
  const order = useStore((s) => s.order)
  const dragHovering = useStore((s) => s.dragHovering)
  const setPendingSend = useStore((s) => s.setPendingSend)
  const receiveCode = useStore((s) => s.receiveCode)
  const openCode = useStore((s) => s.openCode)
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
    // Desktop: any DropBeam code does its one sensible thing (a friend code adds
    // the friend, a folder invite opens Accept invite…). Mobile keeps receive-only.
    const ok = await (MOBILE_UI ? receiveCode(code) : openCode(code))
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
        {!showReceive ? <button className="ios-button" onClick={() => setShowReceive(true)}><ArrowDownToLine size={18} />Have a code? Receive files</button> :
          <div className="dialog-overlay"><form role="dialog" aria-modal="true" aria-label="Receive files" className="mobile-receive dialog" onSubmit={submitReceive} onKeyDown={e => {
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

  const finished = list.filter((t) => !isActive(t.state) && t.state !== 'paused')
  const clearFinished = () => finished.forEach((t) => useStore.getState().removeTransfer(t.id))

  return (
    <div className="page send-page">
      <div className="page-header titlebar-drag">
        <h1 className="page-title">Send &amp; Receive</h1>
        <div className="page-actions">
          <button className="btn btn-secondary" onClick={() => setShowReceive(true)}>
            <ArrowDownToLine /> Receive…
          </button>
          {!IS_MAC && (
            <button className="btn btn-secondary" disabled={picking} onClick={() => void onPick('folder')} title="Send a folder and everything in it">
              <FolderUp /> Send Folder…
            </button>
          )}
          <button className="btn btn-primary" disabled={picking} onClick={() => void onPick()}>
            <SendIcon /> Send Files…
          </button>
        </div>
      </div>

      {list.length === 0 ? (
        <DropZone hovering={dragHovering} onPick={() => void onPick()} picking={picking} />
      ) : (
        <>
          <DropZone hovering={dragHovering} onPick={() => void onPick()} picking={picking} compact />
          <SectionHeader
            action={finished.length > 0 ? <button className="btn btn-plain btn-sm" onClick={clearFinished}>Clear Finished</button> : undefined}
          >
            Transfers
          </SectionHeader>
          <div className="group xfer-list">
            <AnimatePresence initial={false}>
              {list.map((t) => (
                <TransferCard key={t.id} t={t} />
              ))}
            </AnimatePresence>
          </div>
        </>
      )}

      {showReceive && (
        <Dialog
          title="Receive Files"
          width={420}
          onClose={() => { setShowReceive(false); setCode('') }}
          footer={
            <>
              <button className="btn btn-secondary" type="button" onClick={() => { setShowReceive(false); setCode('') }}>Cancel</button>
              <button className="btn btn-primary" type="submit" form="receive-code-form" disabled={!code.trim()}>Receive</button>
            </>
          }
        >
          <form id="receive-code-form" onSubmit={submitReceive}>
            <label className="field-label" htmlFor="receive-code">Paste the code from the sender, or scan their QR code.</label>
            <div className="receive-code-row">
              <input
                id="receive-code"
                className="input mono"
                autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" inputMode="text"
                placeholder="Code"
                aria-label="Receive code"
                value={code}
                autoFocus
                onChange={(e) => setCode(e.target.value)}
              />
              <ScanCodeButton
                label="Scan…"
                hint="Hold the sender’s QR code up to your camera."
                title="Scan to receive"
                accept={['receive']}
                onCode={(value) => { setCode(value); void openCode(value).then((ok) => { if (ok) { setCode(''); setShowReceive(false) } }) }}
                onOther={(p) => void openCode(p.code).then((ok) => { if (ok) { setCode(''); setShowReceive(false) } })}
              />
            </div>
          </form>
        </Dialog>
      )}
    </div>
  )
}
