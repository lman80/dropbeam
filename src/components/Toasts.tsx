import { AnimatePresence, motion } from 'framer-motion'
import { AlertCircle, CheckCircle2, Info, X } from 'lucide-react'
import { useStore } from '../store'
import { IconButton } from './ui'

export function Toasts() {
  const toasts = useStore((s) => s.toasts)
  const dismiss = useStore((s) => s.dismissToast)

  return (
    <div className="app-toasts" role="region" aria-label="Notifications" aria-live="polite">
      <AnimatePresence initial={false}>
        {toasts.map((t) => {
          const Icon = t.kind === 'error' ? AlertCircle : t.kind === 'success' ? CheckCircle2 : Info
          return (
            <motion.div
              key={t.id}
              layout="position"
              initial={{ opacity: 0, y: 8 }}
              animate={{ opacity: 1, y: 0 }}
              exit={{ opacity: 0, transition: { duration: 0.12 } }}
              transition={{ duration: 0.16, ease: [0.2, 0.8, 0.2, 1] }}
              className="toast"
              role={t.kind === 'error' ? 'alert' : 'status'}
            >
              <Icon className={`toast-icon ${t.kind}`} />
              <div className="toast-msg selectable">{t.message}</div>
              <IconButton label="Dismiss" size="sm" onClick={() => dismiss(t.id)}>
                <X />
              </IconButton>
            </motion.div>
          )
        })}
      </AnimatePresence>
    </div>
  )
}
