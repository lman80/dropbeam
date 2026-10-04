/* eslint-disable react-refresh/only-export-components -- useEscape is shared with the dialog */
// The one dialog pattern for the desktop app: dimmed/blurred overlay, a
// surface panel with a title row (title + optional subtitle + close ×), a
// scrolling body and an optional footer of actions. Escape closes the TOPMOST
// dialog only; a click on the backdrop closes it too (unless busy).
import { useEffect, useId, useLayoutEffect, useRef, useState, type CSSProperties, type ReactNode, type RefObject } from 'react'
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

// ── Modal focus: trap, inert background, initial focus, restore ──────────────
const FOCUSABLE = 'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), [contenteditable="true"]'
const modalStack: HTMLElement[] = []
/** How many open modals made each background element inert (nested dialogs). */
const inertCount = new WeakMap<Element, number>()

function focusables(root: HTMLElement): HTMLElement[] {
  return [...root.querySelectorAll<HTMLElement>(FOCUSABLE)].filter((el) => el.getClientRects().length > 0 && !el.closest('[inert]'))
}

/** Everything outside `el` (siblings of it and of each ancestor up to <body>),
 *  except live surfaces that must stay usable: toasts, tooltips, menus. */
function backgroundOf(el: HTMLElement): Element[] {
  const out: Element[] = []
  let node: HTMLElement | null = el
  while (node && node.parentElement && node !== document.body) {
    for (const sib of node.parentElement.children) {
      if (sib === node || sib.tagName === 'SCRIPT' || sib.tagName === 'STYLE') continue
      if (sib.matches('.app-toasts, .tooltip, .menu, .popover-panel-ui')) continue
      out.push(sib)
    }
    node = node.parentElement
  }
  return out
}

/**
 * The modal contract every dialog-shaped surface shares: on open, remember what
 * had focus, make the rest of the window inert (no Tab, click or screen-reader
 * escape), and move focus inside (an autofocus field, else the first field, else
 * the default button, else the panel); Tab cycles within; on close, the
 * background comes back and focus returns where it was.
 */
export function useModalFocus(panelRef: RefObject<HTMLElement | null>, overlayRef?: RefObject<HTMLElement | null>) {
  // Captured during the first render — before an autoFocus child steals focus.
  const [opener] = useState(() => (typeof document === 'undefined' ? null : document.activeElement as HTMLElement | null))
  useLayoutEffect(() => {
    const panel = panelRef.current
    if (!panel || MOBILE_UI) return
    const root = overlayRef?.current ?? panel
    const previous = opener
    modalStack.push(panel)
    const bg = backgroundOf(root)
    for (const el of bg) {
      const n = inertCount.get(el) ?? 0
      inertCount.set(el, n + 1)
      if (n === 0) el.setAttribute('inert', '')
    }
    // Initial focus (an element that autofocused itself already won).
    if (!panel.contains(document.activeElement)) {
      const target =
        panel.querySelector<HTMLElement>('[autofocus], [data-autofocus]') ??
        panel.querySelector<HTMLElement>('.dialog-body :is(input, textarea, select):not([disabled])') ??
        panel.querySelector<HTMLElement>('.dialog-footer .btn-primary:not([disabled])') ??
        focusables(panel).find((el) => !el.closest('.dialog-head')) ??
        panel
      if (target === panel && !panel.hasAttribute('tabindex')) panel.setAttribute('tabindex', '-1')
      target.focus({ preventScroll: true })
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key !== 'Tab' || modalStack[modalStack.length - 1] !== panel) return
      const list = focusables(panel)
      if (!list.length) { e.preventDefault(); panel.focus(); return }
      const first = list[0]
      const last = list[list.length - 1]
      const active = document.activeElement as HTMLElement | null
      if (e.shiftKey && (active === first || !panel.contains(active))) { e.preventDefault(); last.focus() }
      else if (!e.shiftKey && (active === last || !panel.contains(active))) { e.preventDefault(); first.focus() }
    }
    document.addEventListener('keydown', onKey, true)
    return () => {
      document.removeEventListener('keydown', onKey, true)
      const i = modalStack.indexOf(panel)
      if (i >= 0) modalStack.splice(i, 1)
      for (const el of bg) {
        const n = (inertCount.get(el) ?? 1) - 1
        if (n <= 0) { inertCount.delete(el); el.removeAttribute('inert') }
        else inertCount.set(el, n)
      }
      if (previous && previous.isConnected && typeof previous.focus === 'function') previous.focus({ preventScroll: true })
    }
    // Mount/unmount only: the trap follows the panel element itself.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
}

/** Dialogs that a parent unmounts without an AnimatePresence still fade out:
 *  on unmount a non-interactive snapshot of the overlay fades away in its place. */
function useFadeOutGhost(ref: RefObject<HTMLElement | null>) {
  useLayoutEffect(() => {
    const el = ref.current
    if (!el || MOBILE_UI) return
    return () => {
      if (!el.isConnected || window.matchMedia('(prefers-reduced-motion: reduce)').matches) return
      // Already faded by a parent AnimatePresence: nothing left to show.
      if (Number(getComputedStyle(el).opacity) < 0.05) return
      const ghost = el.cloneNode(true) as HTMLElement
      ghost.removeAttribute('id')
      ghost.setAttribute('aria-hidden', 'true')
      ghost.setAttribute('inert', '')
      ghost.classList.add('dialog-ghost')
      document.body.appendChild(ghost)
      requestAnimationFrame(() => ghost.classList.add('leaving'))
      window.setTimeout(() => ghost.remove(), 220)
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [])
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
  const overlayRef = useRef<HTMLDivElement>(null)
  const panelRef = useRef<HTMLDivElement>(null)
  useModalFocus(panelRef, overlayRef)
  useFadeOutGhost(overlayRef)
  return (
    <motion.div
      ref={overlayRef}
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
        ref={panelRef}
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
