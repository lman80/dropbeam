import { useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import type { UnlistenFn } from '@tauri-apps/api/event'
import { AnimatePresence, motion } from 'framer-motion'
import { ArrowDownToLine, Check, Copy, Search, Send, Settings, UserPlus, X } from 'lucide-react'
import { api, HAS_TAURI, isActive, type TransferUpdate } from '../lib/api'
import { useStore } from '../store'
import { Spinner } from '../components/bits'
import { avatarGradient, initials } from '../lib/avatar'
import { friendOnlineState } from '../lib/presence'
import { formatSpeed } from '../lib/format'

const openMain = () => invoke('open_main_window').catch(() => {})
const hideSelf = () => invoke('hide_popover').catch(() => {})

export function Popover() {
  const init = useStore((s) => s.init)
  const ready = useStore((s) => s.ready)
  const friends = useStore((s) => s.friends)
  const friendSeen = useStore((s) => s.friendSeen)
  const folderStatuses = useStore((s) => s.folderStatuses)
  const transfers = useStore((s) => s.transfers)
  const order = useStore((s) => s.order)
  const sendPaths = useStore((s) => s.sendPaths)
  const sendToFriend = useStore((s) => s.sendToFriend)
  const receiveCode = useStore((s) => s.receiveCode)

  const [query, setQuery] = useState('')
  const [pickingFor, setPickingFor] = useState<string | null>(null)
  const [code, setCode] = useState('')
  const [showReceive, setShowReceive] = useState(false)
  const [dragActive, setDragActive] = useState(false)
  const [dragHoverId, setDragHoverId] = useState<string | null>(null)

  // Row DOM nodes, so we can map a drag's pixel position → the friend under it.
  const rowRefs = useRef<Record<string, HTMLElement | null>>({})

  useEffect(() => {
    init()
  }, [init])

  const filtered = useMemo(() => {
    const q = query.trim().toLowerCase()
    return q ? friends.filter((f) => f.name.toLowerCase().includes(q)) : friends
  }, [friends, query])

  // Which friend row is under this drag position? Tauri reports physical pixels,
  // but to be robust across displays/versions we try the position both as-is and
  // divided by the device pixel ratio, and fall back to the nearest row in the
  // contacts column — so a drop that lands a little off still sends.
  const friendIdAtPoint = (pos: { x: number; y: number }): string | null => {
    const dpr = window.devicePixelRatio || 1
    const candidates = [
      { x: pos.x / dpr, y: pos.y / dpr },
      { x: pos.x, y: pos.y },
    ]
    for (const c of candidates) {
      for (const f of filtered) {
        const el = rowRefs.current[f.id]
        if (!el) continue
        const r = el.getBoundingClientRect()
        if (c.x >= r.left && c.x <= r.right && c.y >= r.top && c.y <= r.bottom) return f.id
      }
    }
    // Near-miss: pick the closest row whose horizontal band the drop is within.
    for (const c of candidates) {
      let best: { id: string; dist: number } | null = null
      for (const f of filtered) {
        const el = rowRefs.current[f.id]
        if (!el) continue
        const r = el.getBoundingClientRect()
        if (c.x < r.left - 24 || c.x > r.right + 24) continue
        const dist = Math.abs(c.y - (r.top + r.bottom) / 2)
        if (!best || dist < best.dist) best = { id: f.id, dist }
      }
      if (best && best.dist < 64) return best.id
    }
    return null
  }

  // Real OS file drags (Tauri) arrive here with a position — map to a friend.
  useEffect(() => {
    if (!HAS_TAURI) return
    let unlisten: UnlistenFn | undefined
    let cancelled = false
    getCurrentWebview()
      .onDragDropEvent((event) => {
        const p = event.payload
        if (p.type === 'enter' || p.type === 'over') {
          setDragActive(true)
          setDragHoverId(friendIdAtPoint(p.position))
        } else if (p.type === 'drop') {
          const id = friendIdAtPoint(p.position)
          setDragActive(false)
          setDragHoverId(null)
          if (id && p.paths?.length) {
            void sendToFriend(id, p.paths)
            // Blip-style: close the menu once the send is on its way (the HUD
            // shows progress). Small delay so the row's "sending" state is seen.
            setTimeout(() => hideSelf(), 450)
          }
        } else {
          setDragActive(false)
          setDragHoverId(null)
        }
      })
      .then((u) => {
        if (cancelled) u()
        else unlisten = u
      })
    return () => {
      cancelled = true
      unlisten?.()
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filtered, sendToFriend])

  const beamToFriend = async (id: string) => {
    setPickingFor(id)
    try {
      const paths = await api.pickFiles()
      if (paths.length) await sendToFriend(id, paths)
    } finally {
      setPickingFor(null)
    }
  }

  const pickAndSend = async () => {
    setPickingFor('__quick__')
    try {
      const paths = await api.pickFiles()
      if (paths.length) sendPaths(paths)
    } finally {
      setPickingFor(null)
    }
  }

  const submitReceive = (e: React.FormEvent) => {
    e.preventDefault()
    if (!code.trim()) return
    receiveCode(code)
    setCode('')
    setShowReceive(false)
  }

  const active = useMemo(
    () =>
      order
        .map((id) => transfers[id])
        .filter(Boolean)
        .filter((t) => isActive(t.state) || t.state === 'completed')
        .reverse()
        .slice(0, 4),
    [order, transfers],
  )

  return (
    <div className="popover-root">
      <div className={`popover-panel${dragActive ? ' dragging' : ''}`}>
        <header className="popover-head">
          <button className="icon-btn" title="Open DropBeam" onClick={openMain}>
            <Settings size={15} />
          </button>
          <span className="popover-title">DropBeam</span>
          <button className="icon-btn" title="Close" onClick={hideSelf}>
            <X size={15} />
          </button>
        </header>

        <div className="pop-search-wrap">
          <Search size={15} className="pop-search-icon" />
          <input
            className="pop-search"
            placeholder="Search friends"
            value={query}
            spellCheck={false}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>

        <div className="popover-body">
          {filtered.length ? (
            <div className="pop-contacts">
              {filtered.map((f) => {
                const online = friendOnlineState(f.name, friendSeen, folderStatuses) === true
                const hot = dragHoverId === f.id
                return (
                  <button
                    key={f.id}
                    ref={(el) => {
                      rowRefs.current[f.id] = el
                    }}
                    className={`pop-contact${hot ? ' drop' : ''}`}
                    onClick={() => beamToFriend(f.id)}
                    disabled={pickingFor !== null}
                    title={`Send files to ${f.name}`}
                    // HTML5 DnD fallback (browser preview + non-OS drags)
                    onDragOver={(e) => {
                      e.preventDefault()
                      setDragActive(true)
                      setDragHoverId(f.id)
                    }}
                    onDragLeave={() =>
                      setDragHoverId((cur) => (cur === f.id ? null : cur))
                    }
                    onDrop={(e) => {
                      e.preventDefault()
                      setDragActive(false)
                      setDragHoverId(null)
                    }}
                  >
                    <span
                      className="pop-contact-av"
                      style={{ background: avatarGradient(f.id) }}
                    >
                      {pickingFor === f.id ? <Spinner size={15} /> : initials(f.name)}
                      {online && <span className="pop-online-dot" />}
                    </span>
                    <span className="pop-contact-text">
                      <span className="pop-contact-name">{f.name}</span>
                      <span className="pop-contact-sub">
                        {hot ? 'Drop to send' : online ? 'Online now' : 'Tap or drop a file'}
                      </span>
                    </span>
                    <Send size={15} className="pop-contact-send" />
                  </button>
                )
              })}
            </div>
          ) : (
            <button
              className="btn btn-ghost"
              style={{ width: '100%', fontSize: 12.5 }}
              onClick={openMain}
            >
              <UserPlus size={14} /> {query ? 'No match — add a friend' : 'Add a friend in the app'}
            </button>
          )}

          {active.length > 0 && (
            <>
              <div className="popover-section-label">Transfers</div>
              <div style={{ display: 'flex', flexDirection: 'column', gap: 7 }}>
                {active.map((t) => (
                  <PopoverTransfer key={t.id} t={t} />
                ))}
              </div>
            </>
          )}

          {!ready && (
            <div style={{ display: 'grid', placeItems: 'center', padding: 16 }}>
              <Spinner size={18} />
            </div>
          )}
        </div>

        <footer className="pop-foot">
          <button
            className="btn btn-primary"
            style={{ flex: 1, justifyContent: 'center' }}
            onClick={pickAndSend}
            disabled={pickingFor === '__quick__'}
          >
            {pickingFor === '__quick__' ? <Spinner size={15} /> : <Send size={15} />} Send a file
          </button>
          <button
            className="btn btn-ghost"
            title="Receive with a code"
            onClick={() => setShowReceive((v) => !v)}
            style={{ color: showReceive ? 'var(--accent)' : undefined }}
          >
            <ArrowDownToLine size={16} />
          </button>
        </footer>
        <AnimatePresence initial={false}>
          {showReceive && (
            <motion.form
              onSubmit={submitReceive}
              initial={{ opacity: 0, height: 0 }}
              animate={{ opacity: 1, height: 'auto' }}
              exit={{ opacity: 0, height: 0 }}
              style={{ overflow: 'hidden', display: 'flex', gap: 8, padding: '0 12px 12px' }}
            >
              <input
                className="input"
                placeholder="Enter a code"
                value={code}
                autoFocus
                spellCheck={false}
                onChange={(e) => setCode(e.target.value)}
                style={{ fontFamily: 'var(--font-mono)', fontSize: 13 }}
              />
              <button className="btn btn-primary" type="submit" disabled={!code.trim()}>
                <ArrowDownToLine size={15} />
              </button>
            </motion.form>
          )}
        </AnimatePresence>
      </div>
    </div>
  )
}

function PopoverTransfer({ t }: { t: TransferUpdate }) {
  const [copied, setCopied] = useState(false)
  const name = t.fileNames[0] ?? (t.direction === 'receive' ? 'Incoming' : 'Files')
  const isSendWaiting =
    t.direction === 'send' && t.state === 'waitingForPeer' && !!t.code && !t.friendName

  const label =
    t.state === 'completed'
      ? t.direction === 'send'
        ? 'Sent'
        : 'Received'
      : t.state === 'transferring'
        ? `${Math.round(t.percent)}% · ${formatSpeed(t.speedBps)}`
        : t.friendName
          ? `to ${t.friendName}`
          : t.direction === 'receive'
            ? 'Receiving…'
            : 'Waiting…'

  const copy = async () => {
    if (!t.code) return
    try {
      await navigator.clipboard.writeText(t.code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1400)
    } catch {
      /* ignore */
    }
  }

  return (
    <div className="popover-xfer">
      <div style={{ display: 'flex', alignItems: 'center', gap: 9 }}>
        <span
          style={{
            width: 7,
            height: 7,
            borderRadius: 999,
            flexShrink: 0,
            background:
              t.state === 'completed'
                ? 'var(--green)'
                : t.state === 'failed'
                  ? 'var(--red)'
                  : 'var(--accent)',
          }}
        />
        <div style={{ flex: 1, minWidth: 0 }}>
          <div
            style={{
              fontSize: 12.5,
              fontWeight: 600,
              whiteSpace: 'nowrap',
              overflow: 'hidden',
              textOverflow: 'ellipsis',
            }}
          >
            {name}
          </div>
          <div style={{ fontSize: 11, color: 'var(--text-faint)' }}>{label}</div>
        </div>
        {isSendWaiting && (
          <button
            className="icon-btn"
            style={{ width: 26, height: 26 }}
            onClick={copy}
            title="Copy code"
          >
            {copied ? <Check size={13} /> : <Copy size={13} />}
          </button>
        )}
      </div>
      {isSendWaiting && <code className="popover-code selectable">{t.code}</code>}
      {t.state === 'transferring' && (
        <div className="popover-progress">
          <div
            className="popover-progress-fill"
            style={{ width: `${Math.max(3, t.percent)}%` }}
          />
        </div>
      )}
    </div>
  )
}
