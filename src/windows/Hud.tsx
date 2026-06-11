import { useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { currentMonitor, getCurrentWindow, LogicalPosition } from '@tauri-apps/api/window'
import { AnimatePresence, motion } from 'framer-motion'
import { ArrowDownToLine, Send, X } from 'lucide-react'
import { HAS_TAURI } from '../lib/api'
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
  const folderStatuses = useStore((s) => s.folderStatuses)
  const pairs = useStore((s) => s.pairs)

  useEffect(() => {
    init()
  }, [init])

  // The single most relevant background activity. One-off sends AND receives now
  // get the Blip-style bottom-right transfer card, so the top HUD is dedicated to
  // shared-folder syncs (the long-running background activity).
  const pill: Pill | null = useMemo(() => {
    // Only surface a folder that is ACTUALLY moving bytes (bytesDone > 0). The
    // folder worker sets state=sending at 0% while it spends ~12s trying to dial
    // a peer; if the peer is offline/unreachable that fails, backs off, and
    // retries — which used to pop this card up over and over at "0%" even though
    // nothing was transferring. Requiring real progress means an offline queue
    // stays silent and the card only appears for a live transfer.
    const folder = Object.values(folderStatuses).find(
      (s) => (s.state === 'sending' || s.state === 'receiving') && s.bytesDone > 0,
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
  }, [folderStatuses, pairs])

  usePositionOnce()

  // Hold the last active pill through brief gaps. The folder sync worker dips to
  // "idle" for a moment between files in a burst, which made `pill` flip to null
  // and back — flickering the whole HUD window off and on. Keep showing the last
  // pill for a short grace period so a multi-file sync stays steady; only when
  // things are genuinely quiet for ~1.8s does the card retract.
  const [shown, setShown] = useState<Pill | null>(null)
  useEffect(() => {
    if (pill) {
      setShown(pill)
      return
    }
    const id = setTimeout(() => setShown(null), 1800)
    return () => clearTimeout(id)
  }, [pill])

  const [dismissed, setDismissed] = useState<string | null>(null)
  // A dismissal sticks only to the activity it was made on. If a *different*
  // item becomes current, show it at once; if everything goes quiet, clear the
  // dismissal after a short grace so the next sync can surface again (brief gaps
  // between files in a burst stay hidden).
  useEffect(() => {
    if (!dismissed) return
    if (shown && shown.key === dismissed) return
    const id = setTimeout(() => setDismissed(null), shown ? 0 : 6000)
    return () => clearTimeout(id)
  }, [shown, dismissed])

  // Drive the native window's visibility from the (debounced) pill.
  const visible = !!shown && shown.key !== dismissed
  useEffect(() => {
    if (!HAS_TAURI) return
    const win = getCurrentWindow()
    if (visible) void win.show()
    else void win.hide()
  }, [visible])

  return (
    <div className="hud-root">
      <AnimatePresence>
        {shown && (
          <motion.div
            key={shown.key}
            className="hud-pill"
            initial={{ opacity: 0, y: -14, scale: 0.9 }}
            animate={{ opacity: 1, y: 0, scale: 1 }}
            exit={{ opacity: 0, y: -14, scale: 0.9 }}
            transition={{ type: 'spring', stiffness: 380, damping: 26 }}
            onClick={() => invoke('open_main_window').catch(() => {})}
          >
            <span className="hud-icon">
              {shown.direction === 'send' ? <Send size={15} /> : <ArrowDownToLine size={15} />}
            </span>
            <span style={{ flex: 1, minWidth: 0 }}>
              <span className="hud-title">{shown.title}</span>
              <span className="hud-sub">{shown.sub}</span>
            </span>
            {shown.locality !== 'unknown' && <ChannelBadge locality={shown.locality} />}
            <span className="hud-ring" aria-hidden>
              {Math.round(shown.percent)}
            </span>
            <button
              className="hud-x"
              title="Dismiss"
              onClick={(e) => {
                e.stopPropagation()
                setDismissed(shown.key)
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
