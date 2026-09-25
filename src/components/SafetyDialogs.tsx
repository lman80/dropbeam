// Block & Report (App Store guideline 1.2), desktop. One host renders whichever
// dialog the store's `safety` prompt asks for, so the friend card, the chat
// header and a message's menu all open the same two dialogs.
import { useEffect, useState, type ReactNode } from 'react'
import { AnimatePresence } from 'framer-motion'
import { Ban, Flag } from 'lucide-react'
import { Dialog } from './Dialog'
import { MenuButton, Spinner, type MenuItem } from './ui'
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
      width={400}
      className="safety-dialog"
      onClose={onClose}
      busy={busy}
      footer={
        <>
          <button className="btn btn-plain" onClick={() => openSafety({ kind: 'report', friendId: friend.id })} disabled={busy}>
            Report…
          </button>
          <span className="spacer" />
          <button className="btn btn-secondary" onClick={onClose} disabled={busy}>Cancel</button>
          <button className="btn btn-destructive" onClick={block} disabled={busy}>
            {busy && <Spinner size={13} />} Block
          </button>
        </>
      }
    >
      <p className="dialog-text safety-text">
        {friend.name} is removed from your friends and can’t message you, send you files or invite you to folders.
        They aren’t told.
      </p>
      <p className="dialog-text safety-text safety-note">
        Your chats stay, and shared folders keep syncing until you leave them. You can unblock in Settings.
      </p>
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
      width={440}
      className="safety-dialog"
      onClose={onClose}
      busy={busy}
      footer={
        <>
          <button className="btn btn-secondary" onClick={onClose} disabled={busy}>Cancel</button>
          <button className="btn btn-primary" onClick={submit} disabled={busy || !reason}>
            {busy && <Spinner size={13} />} Continue to email
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
      <label className="safety-label" htmlFor="report-notes">Anything else? <span className="optional">Optional</span></label>
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
      <p className="safety-foot">Reports go to the DropBeam team by email. You’ll see it before it’s sent.</p>
    </Dialog>
  )
}

/** "…" menu with Report… / Block… for a friend (friend row, chat header).
 *  `before` / `after` add the caller's own items around the safety actions
 *  (the Friends list puts Rename, Invite… and Remove in the same menu). */
export function SafetyMenu({ friend, before = [], after = [], size, blankIcon }: {
  friend: Friend
  before?: MenuItem[]
  after?: MenuItem[]
  size?: 'sm' | 'md'
  /** Replace the Report/Block glyphs (e.g. with an empty checkmark column so
   *  they line up with the caller's items). */
  blankIcon?: ReactNode
}) {
  const openSafety = useStore((s) => s.openSafety)
  const visible = (items: MenuItem[]) => items.some((i) => !i.hidden)
  const items: MenuItem[] = [
    ...before,
    { separator: true, hidden: !visible(before) },
    { label: 'Report…', icon: blankIcon ?? <Flag />, onSelect: () => openSafety({ kind: 'report', friendId: friend.id }) },
    { label: 'Block…', icon: blankIcon ?? <Ban />, danger: true, onSelect: () => openSafety({ kind: 'block', friendId: friend.id }) },
    { separator: true, hidden: !visible(after) },
    ...after,
  ]
  return <MenuButton label={`More options for ${friend.name}`} items={items} size={size} />
}

/** A small "are you sure?" dialog: one line of consequence, Cancel + the
 *  destructive action. */
export function ConfirmDialog({ title, children, confirmLabel, onConfirm, onClose }: {
  title: string
  children?: ReactNode
  confirmLabel: string
  onConfirm: () => void | Promise<unknown>
  onClose: () => void
}) {
  const [busy, setBusy] = useState(false)
  const confirm = async () => {
    setBusy(true)
    try { await onConfirm() } finally { setBusy(false) }
    onClose()
  }
  return (
    <Dialog
      title={title}
      width={380}
      onClose={onClose}
      busy={busy}
      footer={
        <>
          <button className="btn btn-secondary" onClick={onClose} disabled={busy}>Cancel</button>
          <button className="btn btn-destructive" autoFocus onClick={() => void confirm()} disabled={busy}>
            {busy && <Spinner size={13} />} {confirmLabel}
          </button>
        </>
      }
    >
      {children && <p className="dialog-text" style={{ margin: 0 }}>{children}</p>}
    </Dialog>
  )
}
