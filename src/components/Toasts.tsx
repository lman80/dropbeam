import { AnimatePresence, motion } from 'framer-motion'
import { AlertCircle, CheckCircle2, Info, X } from 'lucide-react'
import { useStore } from '../store'

export function Toasts() {
  const toasts = useStore((s) => s.toasts)
  const dismiss = useStore((s) => s.dismissToast)

  return (
    <div
      className="app-toasts"
      style={{
        position: 'fixed',
        bottom: 18,
        right: 18,
        display: 'flex',
        flexDirection: 'column',
        gap: 10,
        zIndex: 300,
        maxWidth: 'min(380px, calc(100vw - 36px))',
      }}
      role="region"
      aria-label="Notifications"
      aria-live="polite"
    >
      <AnimatePresence>
        {toasts.map((t) => {
          const color =
            t.kind === 'error' ? 'var(--red)' : t.kind === 'success' ? 'var(--green)' : 'var(--accent)'
          const Icon = t.kind === 'error' ? AlertCircle : t.kind === 'success' ? CheckCircle2 : Info
          return (
            <motion.div
              key={t.id}
              layout
              initial={{ opacity: 0, x: 40, scale: 0.96 }}
              animate={{ opacity: 1, x: 0, scale: 1 }}
              exit={{ opacity: 0, x: 40, scale: 0.96 }}
              transition={{ type: 'spring', stiffness: 380, damping: 30 }}
              className="card toast"
              role={t.kind === 'error' ? 'alert' : 'status'}
              style={{
                display: 'flex',
                alignItems: 'flex-start',
                gap: 10,
                padding: '11px 12px 11px 14px',
                borderRadius: 'var(--radius-lg)',
                boxShadow: 'var(--shadow-pop)',
                borderLeft: `3px solid ${color}`,
              }}
            >
              <Icon size={18} color={color} style={{ flexShrink: 0, marginTop: 1 }} />
              <div className="selectable" style={{ fontSize: 'var(--font-base)', lineHeight: 1.45, flex: 1, minWidth: 0, overflowWrap: 'anywhere' }}>{t.message}</div>
              <button className="icon-btn icon-btn-sm" aria-label="Dismiss" onClick={() => dismiss(t.id)}>
                <X size={14} />
              </button>
            </motion.div>
          )
        })}
      </AnimatePresence>
    </div>
  )
}
