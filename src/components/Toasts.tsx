import { AnimatePresence, motion } from 'framer-motion'
import { AlertCircle, CheckCircle2, Info, X } from 'lucide-react'
import { useStore } from '../store'

export function Toasts() {
  const toasts = useStore((s) => s.toasts)
  const dismiss = useStore((s) => s.dismissToast)

  return (
    <div
      style={{
        position: 'fixed',
        bottom: 18,
        right: 18,
        display: 'flex',
        flexDirection: 'column',
        gap: 10,
        zIndex: 100,
        maxWidth: 380,
      }}
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
              className="card"
              style={{
                display: 'flex',
                alignItems: 'flex-start',
                gap: 10,
                padding: '12px 14px',
                borderRadius: 14,
              }}
            >
              <Icon size={18} color={color} style={{ flexShrink: 0, marginTop: 1 }} />
              <div style={{ fontSize: 13.5, lineHeight: 1.45, flex: 1 }}>{t.message}</div>
              <button className="icon-btn" style={{ width: 24, height: 24 }} onClick={() => dismiss(t.id)}>
                <X size={14} />
              </button>
            </motion.div>
          )
        })}
      </AnimatePresence>
    </div>
  )
}
