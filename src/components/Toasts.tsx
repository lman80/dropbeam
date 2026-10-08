import { useState } from 'react'
import { AnimatePresence, motion } from 'framer-motion'
import { AlertCircle, CheckCircle2, Info, X } from 'lucide-react'
import { useStore, type Toast } from '../store'
import { IconButton } from './ui'

/** Bottom-right notices. Hovering (or focusing) any toast pauses every
 *  auto-dismiss timer, so a message can be read — or its Details copied —
 *  without racing it. Errors are announced assertively, the rest politely. */
export function Toasts() {
  const toasts = useStore((s) => s.toasts)
  const hold = useStore((s) => s.holdToasts)
  const errors = toasts.filter((t) => t.kind === 'error')
  const notes = toasts.filter((t) => t.kind !== 'error')

  return (
    <div
      className="app-toasts"
      role="region"
      aria-label="Notifications"
      onFocus={() => hold(true)}
      onBlur={(e) => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) hold(false) }}
    >
      <div className="toast-group" aria-live="polite" aria-atomic="false">
        <AnimatePresence initial={false}>
          {notes.map((t) => <ToastItem key={t.id} t={t} />)}
        </AnimatePresence>
      </div>
      <div className="toast-group" aria-live="assertive" aria-atomic="false">
        <AnimatePresence initial={false}>
          {errors.map((t) => <ToastItem key={t.id} t={t} />)}
        </AnimatePresence>
      </div>
    </div>
  )
}

function ToastItem({ t }: { t: Toast }) {
  const dismiss = useStore((s) => s.dismissToast)
  const hold = useStore((s) => s.holdToasts)
  const [open, setOpen] = useState(false)
  const Icon = t.kind === 'error' ? AlertCircle : t.kind === 'success' ? CheckCircle2 : Info
  return (
    <motion.div
      layout="position"
      initial={{ opacity: 0, y: 8 }}
      animate={{ opacity: 1, y: 0 }}
      exit={{ opacity: 0, transition: { duration: 0.12 } }}
      transition={{ duration: 0.16, ease: [0.2, 0.8, 0.2, 1] }}
      className="toast"
      onMouseEnter={() => hold(true)}
      onMouseLeave={() => hold(false)}
    >
      <Icon className={`toast-icon ${t.kind}`} aria-hidden />
      <div className="toast-body">
        <div className="toast-msg selectable">{t.message}</div>
        {t.action && (
          <button type="button" className="btn btn-secondary btn-sm toast-action" onClick={() => { t.action!.run(); dismiss(t.id) }}>
            {t.action.label}
          </button>
        )}
        {t.details && (
          <>
            <button type="button" className="toast-details-btn" aria-expanded={open} onClick={() => setOpen((v) => !v)}>
              {open ? 'Hide Details' : 'Details'}
            </button>
            {open && <pre className="toast-details selectable">{t.details}</pre>}
          </>
        )}
      </div>
      <IconButton label="Dismiss" size="sm" onClick={() => dismiss(t.id)}>
        <X />
      </IconButton>
    </motion.div>
  )
}
