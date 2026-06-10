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
  Minus,
  Send as SendIcon,
  X,
} from 'lucide-react'
import { HAS_TAURI, api, type TransferUpdate } from '../lib/api'
import { useStore } from '../store'

// Pick a file-type glyph from the extension (audio waveform, image, video…).
function iconFor(name: string) {
  const ext = name.split('.').pop()?.toLowerCase() ?? ''
  if (['mp3', 'wav', 'm4a', 'aac', 'flac', 'ogg', 'aiff'].includes(ext)) return FileAudio
  if (['png', 'jpg', 'jpeg', 'gif', 'webp', 'heic', 'tiff', 'bmp', 'svg'].includes(ext))
    return FileImage
  if (['mp4', 'mov', 'm4v', 'avi', 'mkv', 'webm'].includes(ext)) return FileVideo
  if (['txt', 'md', 'rtf', 'pdf', 'doc', 'docx', 'pages'].includes(ext)) return FileText
  return FileIcon
}

function initialOf(name: string | null | undefined): string {
  const n = (name ?? '').trim()
  return n ? n[0]!.toUpperCase() : '?'
}

// Truncate a long filename in the MIDDLE so the extension stays visible.
function midTruncate(s: string, max = 34): string {
  if (s.length <= max) return s
  const keep = Math.floor((max - 1) / 2)
  return `${s.slice(0, keep)}…${s.slice(s.length - keep)}`
}

interface SaveDir {
  label: string
  path: string // '' = default download folder
}

const SENDING_STATES = ['starting', 'waitingForPeer', 'connecting', 'transferring'] as const

// Window sizes for the expanded card vs. the minimized pill (logical px).
const FULL_W = 360
const FULL_H = 400
const MINI_W = 268
const MINI_H = 86

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

  usePositionBottomRight()

  const [saveDirs, setSaveDirs] = useState<SaveDir[]>([{ label: 'Downloads (default)', path: '' }])
  const [menuOpen, setMenuOpen] = useState(false)
  useEffect(() => {
    if (!HAS_TAURI) return
    void (async () => {
      const out: SaveDir[] = [{ label: 'Downloads (default)', path: '' }]
      const add = async (label: string, fn: () => Promise<string>) => {
        try {
          const p = await fn()
          if (p) out.push({ label, path: p })
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
  useEffect(() => {
    if (!HAS_TAURI) return
    const win = getCurrentWindow()
    if (visible) void win.show()
    else {
      setMenuOpen(false)
      void win.hide()
    }
  }, [visible])

  const closeCard = () => {
    setMenuOpen(false)
    if (cardKey) setDismissedKey(cardKey)
    setJustSent(null)
  }
  const startResize = (e: React.MouseEvent) => {
    e.preventDefault()
    if (HAS_TAURI) void getCurrentWindow().startResizeDragging('SouthEast')
  }

  // Minimize → collapse to a small pill (and shrink the window to match).
  const [minimized, setMinimized] = useState(false)
  const setWinSize = (w: number, h: number) => {
    if (HAS_TAURI) void getCurrentWindow().setSize(new LogicalSize(w, h)).catch(() => {})
  }
  const minimize = () => {
    setMenuOpen(false)
    setMinimized(true)
    setWinSize(MINI_W, MINI_H)
  }
  const restore = () => {
    setMinimized(false)
    setWinSize(FULL_W, FULL_H)
  }
  // A genuinely NEW transfer opens expanded. We track the last key in a ref so the
  // brief null between "sending" and "sent ✓" (same transfer) doesn't pop a
  // minimized card back open.
  const lastKey = useRef<string | null>(null)
  useEffect(() => {
    if (!cardKey || lastKey.current === cardKey) return
    lastKey.current = cardKey
    setMinimized(false)
    setWinSize(FULL_W, FULL_H)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [cardKey])

  const respond = (accept: boolean, dest?: string) => {
    if (!incoming) return
    setMenuOpen(false)
    void api.respondToOffer(incoming.id, accept, dest)
  }

  const chooseFolder = async () => {
    setMenuOpen(false)
    try {
      const dir = await api.pickDirectory()
      if (dir && incoming) void api.respondToOffer(incoming.id, true, dir)
    } catch {
      /* cancelled */
    }
  }

  // ── Render data for whichever card is active ──────────────────────────────
  const t = visible ? active : null
  const sending = !incoming && showSend
  const done = sending && !outgoing && !!justSent // a send that just completed

  const name = t?.fileNames[0] ?? (sending ? 'File' : 'Incoming file')
  const multi = (t?.fileCount ?? 1) > 1
  const Glyph = iconFor(name)
  const pendingOffer = incoming?.state === 'waitingForAccept'
  const pct = done ? 100 : t?.state === 'transferring' ? t.percent : 0
  const ringActive = !pendingOffer && !done // show the moving ring while transferring

  // Progress ring geometry around the avatar.
  const R = 30
  const C = 2 * Math.PI * R

  const friendName = t?.friendName ?? null
  const sub = (() => {
    if (incoming) {
      return incoming.state === 'transferring'
        ? `Receiving… ${Math.round(pct)}%`
        : 'Connecting…'
    }
    // outgoing
    if (done) return friendName ? `Sent to ${friendName}` : 'Sent'
    return friendName ? `Sending to ${friendName}…` : 'Sending…'
  })()

  return (
    <div className="rc-root">
      <AnimatePresence>
        {t && minimized && (
          <motion.div
            key={`mini-${cardKey}`}
            className="rc-mini"
            onClick={restore}
            title="Click to expand"
            initial={{ opacity: 0, y: 10 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 10 }}
            transition={{ type: 'spring', stiffness: 360, damping: 28 }}
          >
            <div className={`rc-mini-av${done ? ' done' : ''}`}>
              {done ? (
                <Check size={16} strokeWidth={3} />
              ) : sending && !friendName ? (
                <SendIcon size={14} />
              ) : (
                initialOf(friendName)
              )}
            </div>
            <div className="rc-mini-text">
              <div className="rc-mini-name">{midTruncate(name, 24)}</div>
              <div className="rc-mini-sub">{incoming || done ? sub : `${Math.round(pct)}%`}</div>
            </div>
            <button
              className="rc-close"
              onClick={(e) => {
                e.stopPropagation()
                closeCard()
              }}
              title="Close"
            >
              <X size={14} />
            </button>
          </motion.div>
        )}
        {t && !minimized && (
          <motion.div
            key={`${t.direction}-${t.id}`}
            className="rc-card"
            initial={{ opacity: 0, y: 16, scale: 0.94 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: 16, scale: 0.94 }}
            transition={{ type: 'spring', stiffness: 360, damping: 28 }}
          >
            {/* Drag handle (move the card) + minimize + close */}
            <div className="rc-top" data-tauri-drag-region>
              <button className="rc-min" onClick={minimize} title="Minimize">
                <Minus size={15} />
              </button>
              <button className="rc-close" onClick={closeCard} title="Close">
                <X size={15} />
              </button>
            </div>
            <div className="rc-art" data-tauri-drag-region>
              <div className="rc-page">
                <Glyph size={40} strokeWidth={1.4} />
                <span className="rc-ext">{name.split('.').pop()?.slice(0, 4).toUpperCase()}</span>
              </div>
              <div className="rc-avatar-wrap">
                <svg width={68} height={68} viewBox="0 0 68 68" className="rc-ring">
                  <circle cx={34} cy={34} r={R} className="rc-ring-bg" />
                  {ringActive && (
                    <circle
                      cx={34}
                      cy={34}
                      r={R}
                      className="rc-ring-fg"
                      strokeDasharray={C}
                      strokeDashoffset={C * (1 - pct / 100)}
                      transform="rotate(-90 34 34)"
                    />
                  )}
                </svg>
                {done ? (
                  <div className="rc-avatar rc-avatar-done">
                    <Check size={28} strokeWidth={3} />
                  </div>
                ) : (
                  <div className="rc-avatar">
                    {sending && !friendName ? <SendIcon size={24} /> : initialOf(friendName)}
                  </div>
                )}
              </div>
            </div>

            <div className="rc-name" title={name}>
              {midTruncate(name)}
              {multi ? ` +${(t.fileCount ?? 1) - 1} more` : ''}
            </div>
            <div className="rc-from">{incoming ? `From ${friendName || 'someone'}` : sub}</div>

            {pendingOffer ? (
              <div className="rc-actions">
                <button className="rc-btn rc-decline" onClick={() => respond(false)}>
                  Decline
                </button>
                <div className="rc-accept-group">
                  <button className="rc-btn rc-accept" onClick={() => respond(true)}>
                    Accept
                  </button>
                  <button
                    className="rc-btn rc-accept rc-accept-caret"
                    onClick={() => setMenuOpen((v) => !v)}
                    title="Save to…"
                  >
                    <ChevronDown size={14} />
                  </button>
                </div>
                {menuOpen && (
                  <div className="rc-menu">
                    <div className="rc-menu-label">Save to</div>
                    {saveDirs.map((d) => (
                      <button key={d.label} className="rc-menu-item" onClick={() => respond(true, d.path)}>
                        {d.label}
                      </button>
                    ))}
                    <button className="rc-menu-item" onClick={chooseFolder}>
                      Choose…
                    </button>
                  </div>
                )}
              </div>
            ) : done ? (
              <button className="rc-btn rc-done" onClick={() => setJustSent(null)}>
                Done
              </button>
            ) : (
              <div className="rc-status">{incoming ? sub : `${Math.round(pct)}%`}</div>
            )}

            {/* Bottom-right resize grip */}
            <div className="rc-grip" onMouseDown={startResize} title="Drag to resize">
              <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
                <path d="M9 2 L2 9 M9 6 L6 9" stroke="currentColor" strokeWidth="1.2" fill="none" />
              </svg>
            </div>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}

/** Park the card in the bottom-right corner, near the Dock's Downloads stack. */
function usePositionBottomRight() {
  const done = useRef(false)
  useEffect(() => {
    if (!HAS_TAURI || done.current) return
    done.current = true
    void (async () => {
      try {
        const mon = await currentMonitor()
        if (!mon) return
        const scale = mon.scaleFactor
        const screenW = mon.size.width / scale
        const screenH = mon.size.height / scale
        const originX = mon.position.x / scale
        const originY = mon.position.y / scale
        const w = FULL_W
        const h = FULL_H
        const x = originX + Math.max(8, screenW - w - 20)
        const y = originY + Math.max(8, screenH - h - 70) // above the Dock
        await getCurrentWindow().setPosition(new LogicalPosition(x, y))
      } catch {
        /* best-effort */
      }
    })()
  }, [])
}
