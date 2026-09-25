import { useEffect, useMemo, useState } from 'react'
import { invoke } from '@tauri-apps/api/core'
import { currentMonitor, getCurrentWindow, LogicalPosition } from '@tauri-apps/api/window'
import { AnimatePresence, motion } from 'framer-motion'
import { ArrowDown, ArrowUp, X } from 'lucide-react'
import { HAS_TAURI } from '../lib/api'
import { folderLabel } from '../lib/humanize'
import { IconButton } from '../components/ui'
import { useStore } from '../store'

interface Pill {
  key: string
  direction: 'send' | 'receive'
  title: string
  sub: string
  percent: number
}

export function Hud() {
  const init = useStore((s) => s.init)
  const folderStatuses = useStore((s) => s.folderStatuses)
  const pairs = useStore((s) => s.pairs)
  const showSyncPopup = useStore((s) => s.settings?.showSyncPopup ?? true)

  useEffect(() => {
    init()
  }, [init])

  // The single most relevant background activity. One-off sends AND receives now
  // get the Blip-style bottom-right transfer card, so the top HUD is dedicated to
  // shared-folder syncs (the long-running background activity).
  const pill: Pill | null = useMemo(() => {
    // The user can turn the folder-sync popup off entirely (Settings → it can be
    // distracting during a long sync). When off, the HUD never surfaces folders.
    if (!showSyncPopup) return null
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
      const who = folder.peerName || pair?.peerName || null
      const sending = folder.state === 'sending'
      const toWho = who ? `${sending ? 'to' : 'from'} ${who}` : sending ? 'Sending' : 'Receiving'
      const folderTitle = pair?.folder ? folderLabel(pair.folder) : who ? `Folder ${toWho}` : 'Shared folder'
      const total = folder.sessionTotalFiles ?? 0
      if (total > 1) {
        // A whole folder drop = ONE bar. Overall % = files already done plus the
        // current file's fraction, spread across the batch — so it climbs once to
        // 100% instead of resetting per file.
        const done = folder.sessionDoneFiles ?? 0
        const overall = Math.min(100, ((done + folder.percent / 100) / total) * 100)
        return {
          key: folder.pairId,
          direction: sending ? 'send' : 'receive',
          title: folderTitle,
          sub: `${Math.min(done + 1, total)} of ${total} files ${who ? toWho : ''}`.trim(),
          percent: overall,
        }
      }
      return {
        key: folder.pairId,
        direction: sending ? 'send' : 'receive',
        title: folder.sendingFile ? folderLabel(folder.sendingFile) : folderTitle,
        sub: folder.sendingFile ? `${folderTitle} · ${toWho}` : `Syncing ${toWho}`,
        percent: folder.percent,
      }
    }
    return null
  }, [folderStatuses, pairs, showSyncPopup])

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

  const R = 12
  const C = 2 * Math.PI * R
  return (
    <div className="hud-root">
      <AnimatePresence>
        {shown && (
          <motion.div
            key={shown.key}
            className="hud-pill"
            role="status"
            initial={{ opacity: 0, y: -6 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: -6 }}
            transition={{ duration: 0.16, ease: [0.2, 0.8, 0.2, 1] }}
            onClick={() => invoke('open_main_window').catch(() => {})}
          >
            <span className="hud-ring" aria-hidden>
              <svg width={28} height={28} viewBox="0 0 28 28">
                <circle cx={14} cy={14} r={R} className="hud-ring-track" />
                <circle
                  cx={14}
                  cy={14}
                  r={R}
                  className="hud-ring-fill"
                  strokeDasharray={C}
                  strokeDashoffset={C * (1 - Math.max(0, Math.min(100, shown.percent)) / 100)}
                  transform="rotate(-90 14 14)"
                />
              </svg>
              {shown.direction === 'send' ? <ArrowUp /> : <ArrowDown />}
            </span>
            <span className="hud-text">
              <span className="hud-title" title={shown.title}>{shown.title}</span>
              <span className="hud-sub">{shown.sub}</span>
            </span>
            <span className="hud-pct tnum">{Math.round(shown.percent)}%</span>
            <IconButton
              size="sm"
              label="Dismiss"
              className="hud-x"
              tooltip={null}
              onClick={(e) => {
                e.stopPropagation()
                setDismissed(shown.key)
              }}
            >
              <X />
            </IconButton>
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
        const hudW = 384
        // Tuck it under the menu-bar tray icon, in the top-right corner. The
        // window is wider than the pill (transparent margin for the shadow), so
        // nudge right by that margin to keep the visible pill in the same spot.
        const x = originX + Math.max(8, screenW - hudW + 2)
        const y = originY + 2
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
