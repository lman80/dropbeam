/* eslint-disable react-refresh/only-export-components -- useEscape is shared with the dialog */
// The one dialog pattern for the desktop app: dimmed/blurred overlay, a
// surface panel with a title row (title + optional subtitle + close ×), a
// scrolling body and an optional footer of actions. Escape closes the TOPMOST
// dialog only; a click on the backdrop closes it too (unless busy).
import { useEffect, useId, useRef, type CSSProperties, type ReactNode } from 'react'
import { motion } from 'framer-motion'
import { X } from 'lucide-react'
import { MOBILE_UI } from '../lib/platform'
import { IconButton } from './ui'

type Entry = { close: () => void }
const stack: Entry[] = []
let installed = false
function installEscape() {
  if (installed || typeof window === 'undefined') return
  installed = true
  window.addEventListener('keydown', (e) => {
    if (e.key !== 'Escape' || e.defaultPrevented || e.isComposing) return
    const top = stack[stack.length - 1]
    if (!top) return
    e.preventDefault()
    top.close()
  })
}

/** Close on Escape. Nested dialogs stack: only the most recently opened one
 *  reacts, so Esc peels them off one at a time. Pass `undefined` to disable
 *  (e.g. while a request is in flight). */
export function useEscape(onClose: (() => void) | undefined) {
  const ref = useRef(onClose)
  useEffect(() => {
    ref.current = onClose
  })
  const enabled = !!onClose
  useEffect(() => {
    if (!enabled) return
    installEscape()
    const entry: Entry = { close: () => ref.current?.() }
    stack.push(entry)
    return () => {
      const i = stack.indexOf(entry)
      if (i >= 0) stack.splice(i, 1)
    }
  }, [enabled])
}

export function Dialog({
  title,
  subtitle,
  icon,
  onClose,
  busy = false,
  width = 440,
  footer,
  children,
  className,
  style,
  bodyStyle,
  ariaLabel,
}: {
  title?: ReactNode
  subtitle?: ReactNode
  icon?: ReactNode
  onClose?: () => void
  /** While busy the dialog can't be dismissed (Esc / backdrop / ×). */
  busy?: boolean
  width?: number
  footer?: ReactNode
  children?: ReactNode
  className?: string
  style?: CSSProperties
  bodyStyle?: CSSProperties
  ariaLabel?: string
}) {
  const titleId = useId()
  const close = onClose && !busy ? onClose : undefined
  useEscape(close)
  return (
    <motion.div
      className="dialog-overlay"
      initial={{ opacity: 0 }}
      animate={{ opacity: 1 }}
      exit={{ opacity: 0 }}
      transition={{ duration: 0.14 }}
      // mousedown (not click) so a text selection dragged out of the panel
      // doesn't dismiss the dialog when the mouse is released over the backdrop.
      onMouseDown={(e) => { if (e.target === e.currentTarget) close?.() }}
    >
      <motion.div
        role="dialog"
        aria-modal="true"
        aria-labelledby={title ? titleId : undefined}
        aria-label={title ? undefined : ariaLabel}
        className={MOBILE_UI ? `dialog mobile-sheet ${className ?? ''}` : `dialog dialog-panel ${className ?? ''}`}
        style={MOBILE_UI ? style : { width, ...style }}
        initial={MOBILE_UI ? false : { opacity: 0, scale: 0.985 }}
        animate={MOBILE_UI ? { opacity: 1 } : { opacity: 1, scale: 1 }}
        exit={MOBILE_UI ? { opacity: 0 } : { opacity: 0, scale: 0.99 }}
        transition={{ duration: 0.16, ease: [0.2, 0.8, 0.2, 1] }}
      >
        {(title || onClose) && (
          <div className="dialog-head">
            <div style={{ minWidth: 0 }}>
              {/* Dialogs no longer carry an icon tile; `icon` is accepted for
                  older call sites and only used on the phone layout. */}
              {MOBILE_UI && icon && <span className="dialog-icon">{icon}</span>}
              {title && <h2 id={titleId} className="dialog-title">{title}</h2>}
              {subtitle && <p className="dialog-subtitle">{subtitle}</p>}
            </div>
            {onClose && (
              <IconButton label="Close" tooltip="Close (Esc)" disabled={busy} onClick={onClose}>
                <X />
              </IconButton>
            )}
          </div>
        )}
        <div className="dialog-body" style={bodyStyle}>{children}</div>
        {footer && <div className="dialog-actions dialog-footer">{footer}</div>}
      </motion.div>
    </motion.div>
  )
}
