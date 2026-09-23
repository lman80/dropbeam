// Block & Report (App Store guideline 1.2), desktop. One host renders whichever
// dialog the store's `safety` prompt asks for, so the friend card, the chat
// header and a message's menu all open the same two dialogs.
import { useEffect, useRef, useState, type CSSProperties } from 'react'
import { createPortal } from 'react-dom'
import { AnimatePresence } from 'framer-motion'
import { Ban, Flag, MoreHorizontal } from 'lucide-react'
import { Dialog } from './Dialog'
import { Spinner } from './bits'
import { api, type ChatMessage, type Friend } from '../lib/api'
import { useStore } from '../store'
import { REPORT_REASONS, platformLabel, reportMailto, type ReportReason } from '../lib/report'

export function SafetyDialogHost() {
  const safety = useStore((s) => s.safety)
  const close = useStore((s) => s.closeSafety)
  const friend = useStore((s) => (s.safety ? s.friends.find((f) => f.id === s.safety?.friendId) : undefined))
  const message = useStore((s) =>
    s.safety?.messageId ? (s.chats[s.safety.friendId] ?? []).find((m) => m.id === s.safety?.messageId) : undefined,
  )
  // The friend vanished (blocked/removed on another device): nothing to act on.
  useEffect(() => {
    if (safety && !friend) close()
  }, [safety, friend, close])
  return (
    <AnimatePresence>
      {safety && friend && safety.kind === 'block' && <BlockDialog key="block" friend={friend} onClose={close} />}
      {safety && friend && safety.kind === 'report' && (
        <ReportDialog key={`report-${safety.messageId ?? ''}`} friend={friend} message={message} onClose={close} />
      )}
    </AnimatePresence>
  )
}

function BlockDialog({ friend, onClose }: { friend: Friend; onClose: () => void }) {
  const blockFriend = useStore((s) => s.blockFriend)
  const openSafety = useStore((s) => s.openSafety)
  const [busy, setBusy] = useState(false)
  const block = async () => {
    setBusy(true)
    const ok = await blockFriend(friend.id)
    setBusy(false)
    if (ok) onClose()
  }
  return (
    <Dialog
      title={`Block ${friend.name}?`}
      icon={<Ban size={19} />}
      width={440}
      onClose={onClose}
      busy={busy}
      footer={
        <>
          <button className="btn btn-ghost" onClick={() => openSafety({ kind: 'report', friendId: friend.id })} disabled={busy}>
            <Flag size={14} /> Report instead…
          </button>
          <span style={{ flex: 1 }} />
          <button className="btn btn-ghost" onClick={onClose} disabled={busy}>Cancel</button>
          <button className="btn btn-danger" onClick={block} disabled={busy}>
            {busy ? <Spinner size={14} /> : <Ban size={14} />} Block
          </button>
        </>
      }
    >
      <ul className="safety-points">
        <li>{friend.name} is removed from your friends on all your linked devices.</li>
        <li>They can’t message you, send you files, invite you to folders or browse your Locations — even if they have your code.</li>
        <li>They aren’t told. To them it looks like you’re not accepting.</li>
        <li>Your chat history stays on this device. Unblock any time in Settings → Privacy &amp; safety.</li>
      </ul>
      <p className="dialog-text safety-note">Shared folders you’re both in keep syncing until you leave them in Shared Folders.</p>
    </Dialog>
  )
}

function ReportDialog({ friend, message, onClose }: { friend: Friend; message?: ChatMessage; onClose: () => void }) {
  const blockFriend = useStore((s) => s.blockFriend)
  const toast = useStore((s) => s.toast)
  const [reason, setReason] = useState<ReportReason | ''>('')
  const [includeText, setIncludeText] = useState(true)
  const [notes, setNotes] = useState('')
  const [alsoBlock, setAlsoBlock] = useState(false)
  const [busy, setBusy] = useState(false)
  const version = useStore((s) => s.appVer)
  const isFile = message?.kind === 'file' && !message.gif
  const text = message && !message.deleted && message.kind === 'text' && !message.gif ? message.text.trim() : ''
  const subject: 'person' | 'message' | 'file' = message ? (isFile ? 'file' : 'message') : 'person'

  const submit = async () => {
    if (!reason) return
    setBusy(true)
    try {
      const url = reportMailto({
        reason,
        subject,
        personName: friend.name,
        personId: friend.endpointId,
        messageText: includeText && text ? text : null,
        fileNames: includeText && isFile ? message?.files : null,
        messageTs: message?.ts ?? null,
        notes,
        blocked: alsoBlock,
        appVersion: version || null,
        platform: platformLabel(typeof navigator === 'undefined' ? '' : navigator.userAgent),
      })
      await api.openMailto(url)
      if (alsoBlock) await blockFriend(friend.id)
      toast('success', 'Your email is ready — send it to finish the report. We respond within 24 hours.')
      onClose()
    } catch (e) {
      toast('error', `Couldn’t open your mail app: ${String(e)}`)
    } finally {
      setBusy(false)
    }
  }

  return (
    <Dialog
      title={subject === 'person' ? `Report ${friend.name}` : subject === 'file' ? 'Report this file' : 'Report this message'}
      subtitle="Reports go to the DropBeam team by email. You’ll see the email before it’s sent."
      icon={<Flag size={19} />}
      width={480}
      onClose={onClose}
      busy={busy}
      footer={
        <>
          <button className="btn btn-ghost" onClick={onClose} disabled={busy}>Cancel</button>
          <button className="btn btn-primary" onClick={submit} disabled={busy || !reason}>
            {busy ? <Spinner size={14} /> : null} Continue to email
          </button>
        </>
      }
    >
      <fieldset className="safety-reasons">
        <legend className="safety-label">What’s wrong?</legend>
        {REPORT_REASONS.map((r) => (
          <label key={r.id} className={`safety-reason${reason === r.id ? ' on' : ''}`}>
            <input type="radio" name="report-reason" value={r.id} checked={reason === r.id} onChange={() => setReason(r.id)} />
            {r.label}
          </label>
        ))}
      </fieldset>
      {(text || isFile) && (
        <label className="safety-check">
          <input type="checkbox" checked={includeText} onChange={(e) => setIncludeText(e.target.checked)} />
          <span>
            {isFile ? 'Include the file name' + ((message?.files.length ?? 0) > 1 ? 's' : '') : 'Include the message text'}
            <span className="safety-quote">{isFile ? message?.files.join(', ') : text}</span>
            {isFile && <span className="safety-hint">The file itself is never sent.</span>}
          </span>
        </label>
      )}
      <label className="safety-label" htmlFor="report-notes">Anything else? <span className="safety-hint">(optional)</span></label>
      <textarea
        id="report-notes"
        className="input safety-notes"
        rows={3}
        maxLength={2000}
        value={notes}
        onChange={(e) => setNotes(e.target.value)}
        placeholder="What happened, and when"
      />
      <label className="safety-check">
        <input type="checkbox" checked={alsoBlock} onChange={(e) => setAlsoBlock(e.target.checked)} />
        <span>Also block {friend.name}</span>
      </label>
    </Dialog>
  )
}

/** "⋯" menu with Report… / Block… for a friend (friend card, chat header).
 *  Rendered in a portal at a fixed position so a card's rounded clipping
 *  (overflow: hidden) can't cut it off; flips above the button near the bottom. */
export function SafetyMenu({ friend }: { friend: Friend }) {
  const openSafety = useStore((s) => s.openSafety)
  const [pos, setPos] = useState<CSSProperties | null>(null)
  const btn = useRef<HTMLButtonElement>(null)
  const menu = useRef<HTMLDivElement>(null)
  const open = pos !== null
  useEffect(() => {
    if (!open) return
    const down = (e: MouseEvent) => {
      const t = e.target as Node
      if (!menu.current?.contains(t) && !btn.current?.contains(t)) setPos(null)
    }
    const key = (e: KeyboardEvent) => { if (e.key === 'Escape') setPos(null) }
    const away = () => setPos(null)
    window.addEventListener('mousedown', down)
    window.addEventListener('keydown', key)
    window.addEventListener('resize', away)
    window.addEventListener('scroll', away, true)
    return () => {
      window.removeEventListener('mousedown', down)
      window.removeEventListener('keydown', key)
      window.removeEventListener('resize', away)
      window.removeEventListener('scroll', away, true)
    }
  }, [open])
  const toggle = () => {
    if (open) return setPos(null)
    const r = btn.current?.getBoundingClientRect()
    if (!r) return
    const right = Math.max(8, window.innerWidth - r.right)
    setPos(r.bottom + 110 > window.innerHeight ? { right, top: 'auto', bottom: window.innerHeight - r.top + 6 } : { right, top: r.bottom + 6, bottom: 'auto' })
  }
  const pick = (fn: () => void) => () => { setPos(null); fn() }
  return (
    <>
      <button
        ref={btn}
        className="icon-btn"
        aria-label={`More options for ${friend.name}`}
        aria-haspopup="menu"
        aria-expanded={open}
        title="More"
        onClick={toggle}
      >
        <MoreHorizontal size={16} />
      </button>
      {pos && createPortal(
        <div ref={menu} className="chat-menu safety-menu" role="menu" style={pos}>
          <button role="menuitem" onClick={pick(() => openSafety({ kind: 'report', friendId: friend.id }))}>
            <Flag size={14} /> Report…
          </button>
          <button role="menuitem" className="danger" onClick={pick(() => openSafety({ kind: 'block', friendId: friend.id }))}>
            <Ban size={14} /> Block…
          </button>
        </div>,
        document.body,
      )}
    </>
  )
}
