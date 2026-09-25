import { useEffect, useMemo, useRef, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWebview } from '@tauri-apps/api/webview'
import { emit, listen, type UnlistenFn } from '@tauri-apps/api/event'
import {
  AppWindow,
  ArrowDown,
  ArrowDownToLine,
  ArrowRight,
  ArrowUp,
  Check,
  Copy,
  Power,
  QrCode,
  ScanLine,
  Search,
  Send,
  Settings,
} from 'lucide-react'
import { api, HAS_TAURI, isActive, type TransferUpdate } from '../lib/api'
import { useStore } from '../store'
import { IconButton, MenuButton, ProgressBar, Spinner } from '../components/ui'
import { QrScanner } from '../components/QrScanner'
import { QrCodeView } from '../components/CodeQr'
import { FriendAvatar } from '../components/FriendAvatar'
import { parseCode } from '../lib/codes'
import { Toasts } from '../components/Toasts'
import { avatarColor } from '../lib/avatar'
import { friendPresence, presenceLabel } from '../lib/presence'
import { peerLabel } from '../lib/humanize'
import { formatSpeed as formatSpeedValue } from '../lib/format'

const openMain = () => invoke('open_main_window').catch(() => {})
const hideSelf = () => invoke('hide_popover').catch(() => {})
const quitApp = () => invoke('quit_app').catch(() => {})
// The main window listens for tray actions (the Linux tray menu uses the same path).
const openSettings = () => {
  void emit('dropbeam://tray-action', 'settings').catch(() => {})
  void openMain()
}

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
  const openCode = useStore((s) => s.openCode)

  const [query, setQuery] = useState('')
  const [pickingFor, setPickingFor] = useState<string | null>(null)
  const [code, setCode] = useState('')
  const [showReceive, setShowReceive] = useState(false)
  const [scanning, setScanning] = useState(false)
  const [dragActive, setDragActive] = useState(false)
  const [dragHoverId, setDragHoverId] = useState<string | null>(null)

  // Row DOM nodes, so we can map a drag's pixel position → the friend under it.
  const rowRefs = useRef<Record<string, HTMLElement | null>>({})

  useEffect(() => {
    init()
  }, [init])

  // Escape closes the menu-bar panel (menus and the receive field handle theirs first).
  useEffect(() => {
    if (!HAS_TAURI) return
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape' && !e.defaultPrevented) void hideSelf() }
    window.addEventListener('keydown', onKey)
    return () => window.removeEventListener('keydown', onKey)
  }, [])

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

  // The native drop view reports the point already in CSS pixels (top-left),
  // so we match directly — no dpr guessing.
  const rowIdAtCss = (x: number, y: number): string | null => {
    for (const f of filtered) {
      const el = rowRefs.current[f.id]
      if (!el) continue
      const r = el.getBoundingClientRect()
      if (x >= r.left && x <= r.right && y >= r.top && y <= r.bottom) return f.id
    }
    let best: { id: string; dist: number } | null = null
    for (const f of filtered) {
      const el = rowRefs.current[f.id]
      if (!el) continue
      const r = el.getBoundingClientRect()
      if (x < r.left - 24 || x > r.right + 24) continue
      const dist = Math.abs(y - (r.top + r.bottom) / 2)
      if (!best || dist < best.dist) best = { id: f.id, dist }
    }
    return best && best.dist < 80 ? best.id : null
  }

  // Native menu-bar drag → the popover's native drop view forwards drag moves and
  // the final drop here (the webview itself can't get drops while inactive).
  useEffect(() => {
    if (!HAS_TAURI) return
    const uns: UnlistenFn[] = []
    let cancelled = false
    const add = (p: Promise<UnlistenFn>) =>
      p.then((u) => (cancelled ? u() : uns.push(u)))
    add(
      listen<[number, number]>('popover://native-drag', (e) => {
        const [x, y] = e.payload
        setDragActive(true)
        setDragHoverId(rowIdAtCss(x, y))
      }),
    )
    add(
      listen<[string[], number, number]>('popover://native-drop', (e) => {
        const [paths, x, y] = e.payload
        const id = rowIdAtCss(x, y)
        invoke('traydrag_debug', {
          msg: `JS native-drop x=${x} y=${y} id=${id ?? 'null'} rows=${filtered.length} paths=${paths?.length ?? 0}`,
        }).catch(() => {})
        setDragActive(false)
        setDragHoverId(null)
        if (id && paths?.length) {
          void sendToFriend(id, paths)
          setTimeout(() => hideSelf(), 450)
        }
      }),
    )
    return () => {
      cancelled = true
      uns.forEach((u) => u())
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filtered, sendToFriend])

  // Report friend-row positions to Rust so the NATIVE drop handler can map a
  // drop to a person and send entirely in Rust (the inactive menu webview can't
  // receive Rust→webview events, but webview→Rust invokes always work). We report
  // on a steady cadence so a spring-opened menu always has fresh rects in Rust.
  useEffect(() => {
    if (!HAS_TAURI) return
    const report = () => {
      const rows = filtered
        .map((f) => {
          const el = rowRefs.current[f.id]
          if (!el) return null
          const r = el.getBoundingClientRect()
          if (r.height === 0) return null
          return { id: f.id, top: r.top, bottom: r.bottom, left: r.left, right: r.right }
        })
        .filter(Boolean)
      if (rows.length) invoke('set_popover_rows', { rows }).catch(() => {})
    }
    const raf = requestAnimationFrame(report)
    // Only poll while the popover is actually VISIBLE — this webview stays alive
    // hidden 24/7, and the 700ms IPC ticked forever (row rects can't change while
    // hidden; the mount/refilter report above still fires the moment it shows).
    const iv = setInterval(() => {
      if (!document.hidden) report()
    }, 700)
    return () => {
      cancelAnimationFrame(raf)
      clearInterval(iv)
    }
  }, [filtered])

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
    if (pickingFor !== null) return
    setPickingFor(id)
    try {
      const paths = await api.pickFiles()
      if (paths.length) await sendToFriend(id, paths)
    } catch (e) {
      useStore.getState().toast('error', String(e))
    } finally {
      setPickingFor(null)
    }
  }

  const pickAndSend = async () => {
    if (pickingFor !== null) return
    setPickingFor('__quick__')
    try {
      const paths = await api.pickFiles()
      if (paths.length) sendPaths(paths)
    } catch (e) {
      useStore.getState().toast('error', String(e))
    } finally {
      setPickingFor(null)
    }
  }

  const submitReceive = (e: React.FormEvent) => {
    e.preventDefault()
    if (!code.trim()) return
    // Only clear + close once the receive actually STARTED. Wiping the pasted code
    // optimistically meant a bad/expired code vanished with no feedback — now the
    // code stays put to fix and the toast (rendered here since the popover got its
    // own toast surface) says what went wrong.
    void submitCode(code)
  }

  // Receive codes and friend codes are handled right here; a folder invite or a
  // device code needs the full window (a folder picker, Settings), so it's
  // handed to the main window.
  const submitCode = async (value: string) => {
    const kind = parseCode(value)?.kind
    let ok: boolean
    if (HAS_TAURI && (kind === 'folderInvite' || kind === 'deviceLink')) {
      await emit('dropbeam://open-code', parseCode(value)!.code).catch(() => {})
      void openMain()
      ok = true
    } else ok = await openCode(value)
    if (ok) {
      setCode('')
      setShowReceive(false)
    }
  }

  // Scanning uses the camera prompt / a file picker, which steal focus and hide
  // the popover — so the main window runs the scanner.
  const startScan = () => {
    if (!HAS_TAURI) { setScanning(true); return }
    void emit('dropbeam://scan-code').catch(() => {})
    void openMain()
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
          <span className="popover-title">DropBeam</span>
          <MenuButton
            size="sm"
            label="DropBeam menu"
            items={[
              { label: 'Open DropBeam', icon: <AppWindow />, onSelect: openMain },
              { label: 'Settings…', icon: <Settings />, onSelect: openSettings },
              { separator: true },
              { label: 'Quit DropBeam', icon: <Power />, onSelect: quitApp },
            ]}
          />
        </header>

        <div className="pop-search search-field">
          <Search />
          <input
            className="input"
            type="search"
            placeholder="Search"
            aria-label="Search friends"
            autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off"
            value={query}
            onChange={(e) => setQuery(e.target.value)}
          />
        </div>

        <div className="popover-body">
          {filtered.length ? (
            <div className="pop-contacts">
              {filtered.map((f) => {
                const presence = friendPresence(f.name, friendSeen, folderStatuses)
                const online = presence.status === 'online'
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
                  >
                    <span className="pop-contact-av" style={{ background: avatarColor(f.id) }}>
                      {pickingFor === f.id ? <Spinner size={12} /> : <FriendAvatar friend={f} />}
                      {online && <span className="pop-online-dot" />}
                    </span>
                    <span className="pop-contact-text">
                      <span className="pop-contact-name">{f.name}</span>
                      <span className="pop-contact-sub">
                        {hot ? 'Drop to send' : online ? 'Online' : presenceLabel(presence)}
                      </span>
                    </span>
                  </button>
                )
              })}
            </div>
          ) : ready ? (
            <div className="pop-empty">
              <span>{query ? 'No friends match.' : 'No friends yet.'}</span>
              <button className="btn btn-plain btn-sm" onClick={openMain}>
                Add a Friend…
              </button>
            </div>
          ) : null}

          {!ready && (
            <div className="pop-loading">
              <Spinner size={16} />
            </div>
          )}
        </div>

        {/* Transfers get their own strip under the list, so the friend rows
            (the drop targets) keep their place while something is sending. */}
        {active.length > 0 && (
          <div className="pop-xfers" aria-label="Transfers" role="region">
            {active.map((t) => (
              <PopoverTransfer key={t.id} t={t} />
            ))}
          </div>
        )}

        {showReceive && (
          <form className="pop-receive" onSubmit={submitReceive}>
            <input
              className="input set-mono"
              autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" inputMode="text"
              placeholder="Paste a code"
              aria-label="Receive code"
              value={code}
              autoFocus
              onChange={(e) => setCode(e.target.value)}
              onKeyDown={(e) => { if (e.key === 'Escape') { e.stopPropagation(); setShowReceive(false) } }}
            />
            <IconButton label="Scan a QR code" tooltip="Scan a QR code (opens DropBeam)" side="top" onClick={startScan}>
              <ScanLine />
            </IconButton>
            <IconButton label="Receive" side="top" type="submit" disabled={!code.trim()} className="pop-receive-go">
              <ArrowRight />
            </IconButton>
          </form>
        )}

        <footer className="pop-foot">
          <button
            className={`btn btn-secondary${showReceive ? ' on' : ''}`}
            aria-expanded={showReceive}
            onClick={() => setShowReceive((v) => !v)}
          >
            <ArrowDownToLine />
            Receive…
          </button>
          <button className="btn btn-primary" onClick={pickAndSend} disabled={pickingFor !== null}>
            {pickingFor === '__quick__' ? <Spinner size={12} /> : <Send />}
            Send File…
          </button>
        </footer>
      </div>
      {/* The popover runs its own store instance, so errors toasted here (failed
          drag-to-send, bad receive code, engine still starting) rendered NOWHERE
          without a local toast surface — files silently never sent. */}
      {scanning && <QrScanner hint="Hold the sender’s QR code up to your camera." validate={(text) => (parseCode(text) ? null : 'That QR code isn’t a DropBeam code.')} onClose={() => setScanning(false)} onResult={value => { setScanning(false); void submitCode(value) }} />}
      <Toasts />
    </div>
  )
}

function PopoverTransfer({ t }: { t: TransferUpdate }) {
  const showMegabits = useStore((s) => s.settings?.showMegabits ?? false)
  const formatSpeed = (bps: number) => formatSpeedValue(bps, showMegabits)

  const [copied, setCopied] = useState(false)
  const [showQr, setShowQr] = useState(false)
  const name = t.fileNames[0] ?? (t.direction === 'receive' ? 'Incoming' : 'Files')
  const more = t.fileCount > 1 ? ` +${t.fileCount - 1}` : ''
  const isSendWaiting =
    t.direction === 'send' && t.state === 'waitingForPeer' && !!t.code && !t.friendName
  const who = t.friendName ?? peerLabel(t.peer)

  const label =
    t.state === 'completed'
      ? t.direction === 'send'
        ? who ? `Sent to ${who}` : 'Sent'
        : who ? `Received from ${who}` : 'Received'
      : t.state === 'transferring'
        ? `${Math.round(t.percent)}% · ${formatSpeed(t.speedBps)}`
        : isSendWaiting
          ? 'Waiting — share the code'
          : who
            ? t.direction === 'send' ? `Waiting for ${who}` : `Connecting to ${who}…`
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
    <div className="pop-xfer">
      <div className="pop-xfer-line">
        <span className="pop-xfer-icon" aria-hidden>
          {t.state === 'completed' ? <Check /> : t.direction === 'send' ? <ArrowUp /> : <ArrowDown />}
        </span>
        <div className="pop-xfer-text">
          <div className="pop-xfer-name" title={t.fileNames.join(', ')}>
            <span className="truncate-1">{name}</span>
            {more && <span className="pop-xfer-more tnum">{more}</span>}
          </div>
          <div className="pop-xfer-sub tnum">{label}</div>
        </div>
        {isSendWaiting && (
          <IconButton
            size="sm"
            label={showQr ? 'Hide QR code' : 'Show QR code'}
            active={showQr}
            aria-pressed={showQr}
            side="top"
            onClick={() => setShowQr((v) => !v)}
          >
            <QrCode />
          </IconButton>
        )}
        {isSendWaiting && (
          <IconButton size="sm" label={copied ? 'Copied' : 'Copy code'} side="top" onClick={copy}>
            {copied ? <Check /> : <Copy />}
          </IconButton>
        )}
      </div>
      {isSendWaiting && showQr && <div className="pop-xfer-qr"><QrCodeView value={t.code!} size={148} hint="Scan with DropBeam" enlarge={false} /></div>}
      {isSendWaiting && !showQr && <code className="pop-xfer-code selectable">{t.code}</code>}
      {t.state === 'transferring' && (
        <div className="pop-xfer-bar">
          <ProgressBar percent={t.percent} label={`${name} progress`} />
        </div>
      )}
    </div>
  )
}
