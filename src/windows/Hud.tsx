import { useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { currentMonitor, getCurrentWindow, LogicalPosition } from '@tauri-apps/api/window'
import { AnimatePresence, motion } from 'framer-motion'
import { ArrowDownToLine, Send, X } from 'lucide-react'
import { HAS_TAURI, isActive } from '../lib/api'
import type { Locality } from '../lib/api'
import { ChannelBadge } from '../components/bits'
import { useStore } from '../store'

interface Pill {
  key: string
  direction: 'send' | 'receive'
  title: string
  sub: string
  percent: number
  locality: Locality
}

export function Hud() {
  const init = useStore((s) => s.init)
  const transfers = useStore((s) => s.transfers)
  const order = useStore((s) => s.order)
  const folderStatuses = useStore((s) => s.folderStatuses)
  const pairs = useStore((s) => s.pairs)

  useEffect(() => {
    init()
  }, [init])

  // The single most relevant thing happening right now: a live quick/friend
  // transfer if any, otherwise a folder sync in progress.
  const pill: Pill | null = useMemo(() => {
    const live = order
      .map((id) => transfers[id])
      .filter(Boolean)
      .filter((t) => isActive(t.state))
      .reverse()
    const t = live[0]
    if (t) {
      const name = t.fileNames[0] ?? (t.direction === 'receive' ? 'Incoming file' : 'File')
      const peer = t.friendName ? (t.direction === 'send' ? ` → ${t.friendName}` : ` ← ${t.friendName}`) : ''
      const sub =
        t.state === 'transferring'
          ? `${Math.round(t.percent)}%${peer}`
          : t.state === 'waitingForAccept'
            ? 'Waiting to accept'
            : t.direction === 'receive'
              ? 'Connecting…'
              : t.friendName
                ? `Sending${peer}`
                : 'Ready to send'
      return {
        key: t.id,
        direction: t.direction,
        title: name,
        sub,
        percent: t.state === 'transferring' ? t.percent : 0,
        locality: t.locality,
      }
    }
    const folder = Object.values(folderStatuses).find(
      (s) => s.state === 'sending' || s.state === 'receiving',
    )
    if (folder) {
      const pair = pairs.find((p) => p.id === folder.pairId)
      const who = folder.peerName || pair?.peerName || 'folder'
      const sending = folder.state === 'sending'
      return {
        key: folder.pairId,
        direction: sending ? 'send' : 'receive',
        title: folder.sendingFile || (sending ? `Syncing to ${who}` : `Syncing from ${who}`),
        sub: `${Math.round(folder.percent)}% · ${sending ? 'to' : 'from'} ${who}`,
        percent: folder.percent,
        locality: folder.locality,
      }
    }
    return null
  }, [order, transfers, folderStatuses, pairs])

  usePositionOnce()

  const [dismissed, setDismissed] = useState<string | null>(null)
  // A dismissal sticks only to the activity it was made on. If a *different*
  // item becomes current, show it at once; if everything goes quiet, clear the
  // dismissal after a short grace so the next sync can surface again (brief gaps
  // between files in a burst stay hidden).
  useEffect(() => {
    if (!dismissed) return
    if (pill && pill.key === dismissed) return
    const id = setTimeout(() => setDismissed(null), pill ? 0 : 6000)
    return () => clearTimeout(id)
  }, [pill, dismissed])

  // Drive the native window's visibility from whether there's anything to show.
  const visible = !!pill && pill.key !== dismissed
  useEffect(() => {
    if (!HAS_TAURI) return
    const win = getCurrentWindow()
    if (visible) void win.show()
    else void win.hide()
  }, [visible])

  return (
    <div className="hud-root">
      <AnimatePresence>
        {pill && (
          <motion.div
            key={pill.key}
            className="hud-pill"
            initial={{ opacity: 0, y: -14, scale: 0.9 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -14, scale: 0.9 }}
            transition={{ type: 'spring', stiffness: 380, damping: 26 }}
            onClick={() => invoke('open_main_window').catch(() => {})}
          >
            <span className="hud-icon">
              {pill.direction === 'send' ? <Send size={15} /> : <ArrowDownToLine size={15} />}
            </span>
            <span style={{ flex: 1, minWidth: 0 }}>
              <span className="hud-title">{pill.title}</span>
              <span className="hud-sub">{pill.sub}</span>
            </span>
            {pill.locality !== 'unknown' && <ChannelBadge locality={pill.locality} />}
            <span className="hud-ring" aria-hidden>
              {Math.round(pill.percent)}
            </span>
            <button
              className="hud-x"
              title="Dismiss"
              onClick={(e) => {
                e.stopPropagation()
                setDismissed(pill.key)
              }}
            >
              <X size={13} />
            </button>
          </motion.div>
        )}
      </AnimatePresence>
    </div>
  )
}

/** Park the HUD near the top-center of the screen, once. */
function usePositionOnce() {
  useEffect(() => {
    if (!HAS_TAURI) return
    let cancelled = false
    void (async () => {
      try {
        const mon = await currentMonitor()
        if (cancelled || !mon) return
        const scale = mon.scaleFactor
        const screenW = mon.size.width / scale
        const originX = mon.position.x / scale
        const originY = mon.position.y / scale
        const hudW = 340
        // Tuck it under the menu-bar tray icon, in the top-right corner.
        const x = originX + Math.max(8, screenW - hudW - 14)
        const y = originY + 8
        await getCurrentWindow().setPosition(new LogicalPosition(x, y))
      } catch {
        /* positioning is best-effort */
      }
    })()
    return () => {
      cancelled = true
    }
  }, [])
}
