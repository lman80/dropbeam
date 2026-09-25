import { useEffect, useMemo, useRef, useState } from 'react'
import { currentMonitor, getCurrentWindow, LogicalPosition, LogicalSize } from '@tauri-apps/api/window'
import { desktopDir, documentDir, downloadDir, homeDir } from '@tauri-apps/api/path'
import { AnimatePresence, motion } from 'framer-motion'
import {
  Check,
  ChevronDown,
  File as FileIcon,
  FileAudio,
  FileImage,
  FileText,
  FileVideo,
  Send as SendIcon,
} from 'lucide-react'
import { HAS_TAURI, api, type TransferUpdate } from '../lib/api'
import { avatarColor } from '../lib/avatar'
import { peerLabel } from '../lib/humanize'
import { MenuPopover, ProgressBar, Spinner, type MenuItem } from '../components/ui'
import { useStore } from '../store'

// Pick a file-type glyph from the extension (audio waveform, image, video…).
function glyphFor(name: string) {
  const ext = name.split('.').pop()?.toLowerCase() ?? ''
  const props = { size: 20, strokeWidth: 1.6 }
  if (['mp3', 'wav', 'm4a', 'aac', 'flac', 'ogg', 'aiff'].includes(ext)) return <FileAudio {...props} />
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'heic', 'tiff', 'bmp', 'svg'].includes(ext)) return <FileImage {...props} />
  if (['mp4', 'mov', 'm4v', 'avi', 'mkv', 'webm'].includes(ext)) return <FileVideo {...props} />
  if (['txt', 'md', 'rtf', 'pdf', 'doc', 'docx', 'pages'].includes(ext)) return <FileText {...props} />
  return <FileIcon {...props} />
}

/** "3 s", "2 min", "1 h 5 min" — or null when there's no useful estimate. */
function etaText(seconds: number | null | undefined): string | null {
  if (seconds == null || !Number.isFinite(seconds) || seconds < 0.5) return null
  if (seconds < 60) return `${Math.ceil(seconds)} s`
  const mins = Math.round(seconds / 60)
  if (mins < 60) return `${mins} min`
  return `${Math.floor(mins / 60)} h ${mins % 60} min`
}

function initialOf(name: string | null | undefined): string {
  const n = (name ?? '').trim()
  return n ? n[0]!.toUpperCase() : '?'
}

// Truncate a long filename in the MIDDLE so the extension stays visible. The
// card is fixed-width and centered, so the name is cut to fit rather than being
// allowed to stretch the layout off-centre (CSS can only ellipsize the end).
function midTruncate(s: string, max = 24): string {
  if (s.length <= max) return s
  const keep = Math.floor((max - 1) / 2)
  return `${s.slice(0, keep)}…${s.slice(s.length - keep)}`
}

interface SaveDir {
  label: string
  path: string // '' = default download folder
}

const SENDING_STATES = ['starting', 'waitingForPeer', 'connecting', 'transferring'] as const

// Card window size (logical px). The window has native macOS traffic-light
// controls (titleBarStyle Overlay) — yellow minimizes it into the Dock, red
// dismisses it — so there's no custom minimize/close chrome anymore. Every
// state (offer, progress, sent) is laid out to fit this one frame exactly.
const FULL_W = 190
const FULL_H = 184

/**
 * The floating Blip-style transfer card (bottom-right, near Downloads).
 *  - INCOMING: shows the offer with Accept / Decline + a "Save to" menu, then a
 *    download ring while it transfers.
 *  - OUTGOING: shows "Sending to <name>" with an upload ring, then "Sent ✓" with a
 *    Done button before it auto-dismisses. The send view only appears when the
 *    main window ISN'T focused (e.g. a menu-bar drag-to-send) — if you're already
 *    in the app, the in-app UI is enough.
 */
export function ReceiveCard() {
  const init = useStore((s) => s.init)
  const transfers = useStore((s) => s.transfers)
  const order = useStore((s) => s.order)
  useEffect(() => {
    init()
  }, [init])

  // The most relevant INCOMING file: a pending offer or an active receive.
  const incoming = useMemo(() => {
    const live = order
      .map((id) => transfers[id])
      .filter(Boolean)
      .filter(
        (t) =>
          t.direction === 'receive' &&
          (t.state === 'waitingForAccept' ||
            t.state === 'transferring' ||
            t.state === 'connecting'),
      )
      .reverse()
    return live[0] ?? null
  }, [order, transfers])

  // The most relevant OUTGOING send in flight.
  const outgoing = useMemo(() => {
    const live = order
      .map((id) => transfers[id])
      .filter(Boolean)
      .filter((t) => t.direction === 'send' && (SENDING_STATES as readonly string[]).includes(t.state))
      .reverse()
    return live[0] ?? null
  }, [order, transfers])

  // Track a send through to completion so we can flash "Sent ✓" briefly. We only
  // celebrate a send the card was actively showing (so opening the app later
  // doesn't resurface an old completed send).
  const shownSendId = useRef<string | null>(null)
  const [justSent, setJustSent] = useState<TransferUpdate | null>(null)
  useEffect(() => {
    if (outgoing) {
      shownSendId.current = outgoing.id
      if (justSent) setJustSent(null)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [outgoing?.id])
  useEffect(() => {
    const id = shownSendId.current
    if (!id) return
    const t = transfers[id]
    if (!t) return
    if (t.state === 'completed') {
      shownSendId.current = null
      setJustSent(t)
    } else if (t.state === 'failed' || t.state === 'canceled') {
      shownSendId.current = null
    }
  }, [transfers])
  // Auto-dismiss the "Sent ✓" celebration after a few seconds.
  useEffect(() => {
    if (!justSent) return
    const h = setTimeout(() => setJustSent(null), 4500)
    return () => clearTimeout(h)
  }, [justSent])

  // The card pops for every send and receive (the user wants it every time, even
  // with the main window open). The one place it would be redundant — dropping a
  // file directly on the main Send page — is handled there, not here.
  const sendCandidate = outgoing ?? justSent

  useCardFrame(FULL_H)

  const [saveDirs, setSaveDirs] = useState<SaveDir[]>([{ label: 'Default folder', path: '' }])
  const [menuAnchor, setMenuAnchor] = useState<{ rect: DOMRect; el: HTMLElement } | null>(null)
  const closeMenu = () => setMenuAnchor(null)
  useEffect(() => {
    if (!HAS_TAURI) return
    void (async () => {
      // The first row (path: '') = "wherever Settings points" — label it with the
      // REAL folder name instead of assuming "Downloads" (the user may have changed
      // it), and skip the shortcut row that duplicates the default's path (the menu
      // used to always show two Downloads rows).
      let defaultPath = ''
      try {
        const s = await api.getSettings()
        defaultPath = s.downloadDir || ''
      } catch {
        /* fall back to the generic label */
      }
      const defaultName = defaultPath.split(/[/\\]/).filter(Boolean).pop() || 'Downloads'
      const out: SaveDir[] = [{ label: `${defaultName} (default)`, path: '' }]
      const add = async (label: string, fn: () => Promise<string>) => {
        try {
          const p = await fn()
          if (p && p !== defaultPath) out.push({ label, path: p })
        } catch {
          /* skip */
        }
      }
      await add('Desktop', desktopDir)
      await add('Documents', documentDir)
      await add('Downloads', downloadDir)
      await add('Home', homeDir)
      setSaveDirs(out)
    })()
  }, [])

  // Decide which card (if any) to show. Incoming wins; otherwise the send card.
  const showSend = !incoming && !!sendCandidate
  const active = incoming ?? (showSend ? sendCandidate : null)
  const cardKey = active ? `${active.direction}-${active.id}` : null
  // The close (✕) button dismisses the current item; a different transfer later
  // shows the card again.
  const [dismissedKey, setDismissedKey] = useState<string | null>(null)
  const visible = !!active && cardKey !== dismissedKey

  const closeCard = () => {
    closeMenu()
    if (cardKey) setDismissedKey(cardKey)
    setJustSent(null)
  }
  // The native-close listener registers ONCE (below), so it must reach the
  // latest closeCard — not the one captured at mount (when cardKey was null).
  const closeCardRef = useRef(closeCard)
  closeCardRef.current = closeCard

  useEffect(() => {
    if (!HAS_TAURI) return
    const win = getCurrentWindow()
    if (visible) {
      // Briefly become a regular app (Dock icon) so the card's native yellow
      // button can minimize it INTO the Dock and you can click it back.
      void api.setCardActive(true)
      void win.unminimize().catch(() => {})
      void win.show()
      return
    }
    closeMenu()
    void win.hide()
    // Debounce dropping the Dock icon: back-to-back transfers shouldn't flap the
    // activation policy off→on→off. Only revert to menu-bar-only after a quiet
    // beat (same idea as the HUD's anti-flicker grace period).
    const id = setTimeout(() => void api.setCardActive(false), 1800)
    return () => clearTimeout(id)
  }, [visible])

  // The native red traffic-light button should DISMISS the card (not destroy the
  // window — it's reused for the next transfer). Intercept the close request and
  // route it through the LATEST closeCard via the ref.
  useEffect(() => {
    if (!HAS_TAURI) return
    let unlisten: (() => void) | undefined
    void getCurrentWindow()
      .onCloseRequested((e) => {
        e.preventDefault()
        closeCardRef.current()
      })
      .then((u) => {
        unlisten = u
      })
    return () => unlisten?.()
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])

  // A genuinely NEW transfer should surface even if the previous card was
  // minimized into the Dock — un-minimize and raise it. A ref guards against the
  // brief null between "sending" and "sent ✓" of the SAME transfer re-popping it.
  const lastKey = useRef<string | null>(null)
  useEffect(() => {
    if (!HAS_TAURI || !cardKey || lastKey.current === cardKey) return
    lastKey.current = cardKey
    const win = getCurrentWindow()
    void win.unminimize().catch(() => {})
    void win.show()
  }, [cardKey])

  const respond = (accept: boolean, dest?: string) => {
    if (!incoming) return
    closeMenu()
    void api.respondToOffer(incoming.id, accept, dest)
  }

  const chooseFolder = async () => {
    closeMenu()
    try {
      const dir = await api.pickDirectory()
      if (dir && incoming) void api.respondToOffer(incoming.id, true, dir)
    } catch {
      /* cancelled */
    }
  }

  // ── Render data for whichever card is active ──────────────────────────────
  const t = visible ? active : null
  const rates = useStore((s) => (t ? s.transferRates[t.id] : undefined))
  const etaMode = useStore((s) => s.etaMode)
  const sending = !incoming && showSend
  const done = sending && !outgoing && !!justSent // a send that just completed

  const name = t?.fileNames[0] ?? (sending ? 'File' : 'Incoming file')
  const extra = (t?.fileCount ?? 1) - 1
  const pendingOffer = incoming?.state === 'waitingForAccept'
  const pct = done ? 100 : t?.state === 'transferring' ? t.percent : 0

  const who = t ? t.friendName ?? peerLabel(t.peer) : null
  const sub = incoming
    ? `From ${who || 'someone nearby'}`
    : done
      ? who ? `Sent to ${who}` : 'Sent'
      : who ? `To ${who}` : 'Sending'
  // One human line under the bar: how far, and how long is left.
  const eta = t?.state === 'transferring'
    ? etaText((etaMode === 'avg' ? rates?.avgEta ?? rates?.liveEta : rates?.liveEta ?? rates?.avgEta) ?? t.etaSeconds)
    : null
  const meter = t?.state === 'transferring'
    ? [`${Math.round(pct)}%`, eta && `${eta} left`].filter(Boolean).join(' · ')
    : t?.state === 'connecting' || t?.state === 'starting'
      ? 'Connecting…'
      : t?.state === 'waitingForPeer'
        ? `Waiting for ${who || 'the other device'}…`
        : 'Starting…'

  const menuItems: MenuItem[] = [
    { heading: 'Save to' },
    ...saveDirs.map((d) => ({ label: d.label, onSelect: () => respond(true, d.path) })),
    { separator: true },
    { label: 'Choose Folder…', onSelect: () => void chooseFolder() },
  ]

  return (
    <div className="rc-root">
      <AnimatePresence>
        {t && (
          <motion.div
            key={`${t.direction}-${t.id}`}
            className="rc-card"
            initial={{ opacity: 0 }}
            animate={{ opacity: 1 }}
            exit={{ opacity: 0 }}
            transition={{ duration: 0.16 }}
          >
            {/* Spacer under the native traffic-light buttons (titleBarStyle
                Overlay puts red/yellow/green here); also a drag handle. */}
            <div className="rc-top" data-tauri-drag-region />
            <div className="rc-art" data-tauri-drag-region aria-hidden>
              <div className="rc-file">{glyphFor(name)}</div>
              <div
                className={`rc-avatar${done ? ' done' : ''}`}
                style={done ? undefined : { background: who ? avatarColor(who) : undefined }}
              >
                {done ? <Check size={13} strokeWidth={3} /> : sending && !who ? <SendIcon size={11} /> : initialOf(who)}
              </div>
            </div>

            <div className="rc-name" title={t.fileNames.join(', ') || name}>
              <span className="rc-name-text">{midTruncate(name, extra > 0 ? 19 : 24)}</span>
              {extra > 0 && <span className="rc-name-more tnum">+{extra}</span>}
            </div>
            <div className="rc-from" title={sub}>{sub}</div>

            <div className="rc-slot">
              {pendingOffer ? (
                <div className="rc-actions">
                  <button className="btn btn-secondary btn-sm" onClick={() => respond(false)}>
                    Decline
                  </button>
                  <div className="rc-accept">
                    <button className="btn btn-primary btn-sm" onClick={() => respond(true)}>
                      Accept
                    </button>
                    <button
                      className="btn btn-primary btn-sm rc-caret"
                      aria-label="Save to…"
                      title="Save to…"
                      aria-haspopup="menu"
                      aria-expanded={!!menuAnchor}
                      onClick={(e) => {
                        const el = e.currentTarget
                        setMenuAnchor(menuAnchor ? null : { rect: el.getBoundingClientRect(), el })
                      }}
                    >
                      <ChevronDown />
                    </button>
                  </div>
                </div>
              ) : done ? (
                <button className="btn btn-secondary btn-sm rc-done" onClick={() => setJustSent(null)}>
                  Done
                </button>
              ) : (
                <div className="rc-progress">
                  <ProgressBar percent={pct} label={`${name} progress`} />
                  <div className="rc-meter tnum">
                    {t.state !== 'transferring' && <Spinner size={10} />}
                    {meter}
                  </div>
                </div>
              )}
            </div>
          </motion.div>
        )}
      </AnimatePresence>
      {menuAnchor && pendingOffer && (
        <MenuPopover
          anchor={menuAnchor.rect}
          trigger={menuAnchor.el}
          items={menuItems}
          onClose={() => setMenuAnchor(null)}
        />
      )}
    </div>
  )
}

/** Size the card to what it is showing and park it in the bottom-right corner,
 *  near the Dock's Downloads stack. Re-runs when the card changes shape (an
 *  offer needs its buttons; a live transfer needs only its three figures), so
 *  the window is never bigger than its contents. */
function useCardFrame(height: number) {
  useEffect(() => {
    if (!HAS_TAURI) return
    void (async () => {
      try {
        const win = getCurrentWindow()
        await win.setSize(new LogicalSize(FULL_W, height))
        const mon = await currentMonitor()
        if (!mon) return
        const scale = mon.scaleFactor
        const screenW = mon.size.width / scale
        const screenH = mon.size.height / scale
        const originX = mon.position.x / scale
        const originY = mon.position.y / scale
        const x = originX + Math.max(8, screenW - FULL_W - 20)
        const y = originY + Math.max(8, screenH - height - 70) // above the Dock
        await win.setPosition(new LogicalPosition(x, y))
      } catch {
        /* best-effort */
      }
    })()
  }, [height])
}
