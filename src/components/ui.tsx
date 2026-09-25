/* eslint-disable react-refresh/only-export-components -- small shared primitives + hooks */
// Shared desktop UI primitives. Every screen builds from these so controls look
// and behave the same everywhere: IconButton (always with a tooltip), an
// overflow Menu, Toggle, Segmented control, list Group/Row, SectionHeader,
// EmptyState, a thin ProgressBar, a native-looking Spinner and a status Dot.
// Styling lives in index.css (the design-system section).
import {
  cloneElement,
  isValidElement,
  useCallback,
  useEffect,
  useLayoutEffect,
  useRef,
  useState,
  type ButtonHTMLAttributes,
  type CSSProperties,
  type ReactElement,
  type ReactNode,
} from 'react'
import { createPortal } from 'react-dom'
import { MoreHorizontal } from 'lucide-react'

// ── Tooltip ─────────────────────────────────────────────────────────────────
// A small portalled label that appears after a short hover delay (like the
// system's), never clipped by an overflow:hidden parent. Keyboard focus shows
// it immediately.
type Side = 'top' | 'bottom'
export function useTooltip(label: ReactNode | undefined, side: Side = 'bottom') {
  const [anchor, setAnchor] = useState<DOMRect | null>(null)
  const timer = useRef<number | undefined>(undefined)
  const show = useCallback((el: HTMLElement, delay: number) => {
    window.clearTimeout(timer.current)
    timer.current = window.setTimeout(() => setAnchor(el.getBoundingClientRect()), delay)
  }, [])
  const hide = useCallback(() => {
    window.clearTimeout(timer.current)
    setAnchor(null)
  }, [])
  useEffect(() => () => window.clearTimeout(timer.current), [])
  const handlers = label
    ? {
        onMouseEnter: (e: React.MouseEvent<HTMLElement>) => show(e.currentTarget, 480),
        onMouseLeave: hide,
        onMouseDown: hide,
        onFocus: (e: React.FocusEvent<HTMLElement>) => { if (e.currentTarget.matches(':focus-visible')) show(e.currentTarget, 0) },
        onBlur: hide,
      }
    : {}
  const node = label && anchor ? <TooltipBubble label={label} anchor={anchor} side={side} /> : null
  return { handlers, node, hide }
}

function TooltipBubble({ label, anchor, side }: { label: ReactNode; anchor: DOMRect; side: Side }) {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const w = el.offsetWidth
    const h = el.offsetHeight
    const gap = 6
    let top = side === 'top' ? anchor.top - h - gap : anchor.bottom + gap
    if (top + h > window.innerHeight - 4) top = anchor.top - h - gap
    if (top < 4) top = anchor.bottom + gap
    const left = Math.min(Math.max(4, anchor.left + anchor.width / 2 - w / 2), window.innerWidth - w - 4)
    setPos({ left, top })
  }, [anchor, side])
  return createPortal(
    <div ref={ref} role="tooltip" className="tooltip" style={pos ? { left: pos.left, top: pos.top } : { left: -9999, top: -9999 }}>
      {label}
    </div>,
    document.body,
  )
}

/** Wrap any single element to give it a tooltip. */
export function Tooltip({ label, side, children }: { label?: ReactNode; side?: Side; children: ReactElement }) {
  const { handlers, node } = useTooltip(label, side)
  if (!isValidElement(children)) return children
  const props = children.props as Record<string, unknown>
  const merged: Record<string, unknown> = {}
  for (const [k, fn] of Object.entries(handlers)) {
    const orig = props[k] as ((e: unknown) => void) | undefined
    merged[k] = (e: unknown) => { orig?.(e); (fn as (e: unknown) => void)(e) }
  }
  return <>{cloneElement(children, merged)}{node}</>
}

// ── IconButton ──────────────────────────────────────────────────────────────
/** An icon-only button. `label` is required: it is both the accessible name and
 *  the tooltip. */
export function IconButton({
  label,
  tooltip,
  size = 'md',
  danger,
  active,
  className,
  children,
  side,
  ...rest
}: {
  label: string
  /** Tooltip text if it should differ from the accessible label (e.g. add a shortcut). */
  tooltip?: string | null
  size?: 'sm' | 'md'
  danger?: boolean
  active?: boolean
  side?: Side
  children: ReactNode
} & Omit<ButtonHTMLAttributes<HTMLButtonElement>, 'children'>) {
  const { handlers, node } = useTooltip(tooltip === null ? undefined : tooltip ?? label, side)
  const cls = ['icon-btn', size === 'sm' ? 'icon-btn-sm' : '', danger ? 'icon-btn-danger' : '', active ? 'on' : '', className ?? '']
    .filter(Boolean)
    .join(' ')
  return (
    <>
      <button
        type="button"
        aria-label={label}
        className={cls}
        {...rest}
        onMouseEnter={(e) => { rest.onMouseEnter?.(e); handlers.onMouseEnter?.(e) }}
        onMouseLeave={(e) => { rest.onMouseLeave?.(e); handlers.onMouseLeave?.() }}
        onMouseDown={(e) => { rest.onMouseDown?.(e); handlers.onMouseDown?.() }}
        onFocus={(e) => { rest.onFocus?.(e); handlers.onFocus?.(e) }}
        onBlur={(e) => { rest.onBlur?.(e); handlers.onBlur?.() }}
      >
        {children}
      </button>
      {node}
    </>
  )
}

// ── Menu ────────────────────────────────────────────────────────────────────
export type MenuItem =
  | { label: string; icon?: ReactNode; onSelect: () => void; danger?: boolean; disabled?: boolean; hidden?: boolean }
  | { separator: true; hidden?: boolean }
  | { heading: string; hidden?: boolean }

/** A popover menu anchored to a rect. Closes on outside click, Escape, scroll,
 *  resize or selection. Arrow keys move focus between items. */
export function MenuPopover({
  anchor,
  items,
  onClose,
  align = 'end',
  trigger,
}: {
  anchor: DOMRect
  items: MenuItem[]
  onClose: () => void
  align?: 'start' | 'end'
  /** The button that opened it — a press on it toggles instead of re-opening. */
  trigger?: HTMLElement | null
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const w = el.offsetWidth
    const h = el.offsetHeight
    let top = anchor.bottom + 4
    if (top + h > window.innerHeight - 8) top = Math.max(8, anchor.top - h - 4)
    let left = align === 'end' ? anchor.right - w : anchor.left
    left = Math.min(Math.max(8, left), window.innerWidth - w - 8)
    setPos({ left, top })
    el.querySelector<HTMLButtonElement>('.menu-item:not(:disabled)')?.focus({ preventScroll: true })
  }, [anchor, align])
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node
      if (!ref.current?.contains(t) && !trigger?.contains(t)) onClose()
    }
    const onKey = (e: KeyboardEvent) => {
      if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); onClose() }
      if (e.key === 'ArrowDown' || e.key === 'ArrowUp') {
        e.preventDefault()
        const list = [...(ref.current?.querySelectorAll<HTMLButtonElement>('.menu-item:not(:disabled)') ?? [])]
        const i = list.indexOf(document.activeElement as HTMLButtonElement)
        const next = e.key === 'ArrowDown' ? list[(i + 1) % list.length] : list[(i - 1 + list.length) % list.length]
        next?.focus()
      }
    }
    const onScroll = (e: Event) => { if (!ref.current?.contains(e.target as Node)) onClose() }
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('keydown', onKey, true)
    window.addEventListener('resize', onClose)
    window.addEventListener('scroll', onScroll, true)
    return () => {
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('keydown', onKey, true)
      window.removeEventListener('resize', onClose)
      window.removeEventListener('scroll', onScroll, true)
    }
  }, [onClose, trigger])
  return createPortal(
    <div ref={ref} className="menu" role="menu" style={pos ?? { left: -9999, top: -9999 }}>
      {items.filter((i) => !i.hidden).map((item, idx) =>
        'separator' in item ? (
          <div key={`s${idx}`} className="menu-sep" role="separator" />
        ) : 'heading' in item ? (
          <div key={`h${idx}`} className="menu-label">{item.heading}</div>
        ) : (
          <button
            key={item.label}
            type="button"
            role="menuitem"
            className={`menu-item${item.danger ? ' danger' : ''}`}
            disabled={item.disabled}
            onClick={() => { onClose(); item.onSelect() }}
          >
            {item.icon}
            <span className="truncate-1">{item.label}</span>
          </button>
        ),
      )}
    </div>,
    document.body,
  )
}

/** The "…" overflow button + its menu. */
export function MenuButton({
  items,
  label = 'More',
  icon,
  size = 'md',
  align = 'end',
  className,
}: {
  items: MenuItem[]
  label?: string
  icon?: ReactNode
  size?: 'sm' | 'md'
  align?: 'start' | 'end'
  className?: string
}) {
  const [anchor, setAnchor] = useState<DOMRect | null>(null)
  const [trigger, setTrigger] = useState<HTMLElement | null>(null)
  const close = useCallback(() => setAnchor(null), [])
  if (!items.some((i) => !i.hidden && !('separator' in i) && !('heading' in i))) return null
  return (
    <>
      <IconButton
        label={label}
        size={size}
        className={className}
        aria-haspopup="menu"
        aria-expanded={!!anchor}
        onClick={(e) => { e.stopPropagation(); setTrigger(e.currentTarget); setAnchor(anchor ? null : e.currentTarget.getBoundingClientRect()) }}
      >
        {icon ?? <MoreHorizontal />}
      </IconButton>
      {anchor && <MenuPopover anchor={anchor} items={items} onClose={close} align={align} trigger={trigger} />}
    </>
  )
}

// ── Toggle & Segmented ──────────────────────────────────────────────────────
export function Toggle({
  on,
  onChange,
  label,
  disabled,
  title,
}: {
  on: boolean
  onChange: (next: boolean) => void
  label: string
  disabled?: boolean
  title?: string
}) {
  return (
    <button
      type="button"
      role="switch"
      aria-checked={on}
      aria-label={label}
      title={title}
      disabled={disabled}
      className={`toggle${on ? ' on' : ''}`}
      onClick={() => onChange(!on)}
    />
  )
}

export function Segmented<T extends string>({
  value,
  options,
  onChange,
  label,
  block,
  role = 'radiogroup',
  className,
}: {
  value: T
  options: { value: T; label: ReactNode; title?: string }[]
  onChange: (v: T) => void
  label: string
  block?: boolean
  /** 'tablist' when the control switches views. */
  role?: 'radiogroup' | 'tablist'
  className?: string
}) {
  const itemRole = role === 'tablist' ? 'tab' : 'radio'
  return (
    <div className={`seg${block ? ' seg-block' : ''}${className ? ` ${className}` : ''}`} role={role} aria-label={label}>
      {options.map((o) => (
        <button
          key={o.value}
          type="button"
          role={itemRole}
          aria-selected={itemRole === 'tab' ? value === o.value : undefined}
          aria-checked={itemRole === 'radio' ? value === o.value : undefined}
          title={o.title}
          className={value === o.value ? 'active' : ''}
          onClick={() => onChange(o.value)}
        >
          {o.label}
        </button>
      ))}
    </div>
  )
}

// ── Layout ──────────────────────────────────────────────────────────────────
export function SectionHeader({
  children,
  count,
  action,
  style,
}: {
  children: ReactNode
  count?: number
  action?: ReactNode
  style?: CSSProperties
}) {
  const title = (
    <h2 className="section-title" style={action ? undefined : style}>
      {children}
      {count != null && count > 0 && <span className="count">{count}</span>}
    </h2>
  )
  if (!action) return title
  return (
    <div className="section-row" style={style}>
      {title}
      {action}
    </div>
  )
}

export function EmptyState({
  icon,
  title,
  hint,
  action,
  style,
}: {
  icon?: ReactNode
  title: string
  hint?: ReactNode
  action?: ReactNode
  style?: CSSProperties
}) {
  return (
    <div className="empty" style={style}>
      {icon && <div className="empty-glyph" aria-hidden>{icon}</div>}
      <div className="empty-title">{title}</div>
      {hint && <div className="empty-hint">{hint}</div>}
      {action && <div className="empty-actions">{action}</div>}
    </div>
  )
}

export function ProgressBar({ percent, tone, label }: { percent: number; tone?: 'paused' | 'failed'; label?: string }) {
  const pct = Math.max(0, Math.min(100, percent))
  return (
    <div
      className={`progress${tone ? ` ${tone}` : ''}`}
      role="progressbar"
      aria-label={label ?? 'Progress'}
      aria-valuemin={0}
      aria-valuemax={100}
      aria-valuenow={Math.round(pct)}
    >
      <span style={{ width: `${pct > 0 ? Math.max(1.5, pct) : 0}%` }} />
    </div>
  )
}

export function Spinner({ size = 14, style }: { size?: number; style?: CSSProperties }) {
  return <span className="spinner" aria-hidden style={{ width: size, height: size, ...style }} />
}

export function Dot({ tone, title }: { tone?: 'online' | 'ok' | 'busy' | 'warn' | 'error' | 'off'; title?: string }) {
  return <span className={`dot${tone ? ` ${tone}` : ''}`} title={title} aria-hidden={!title} />
}

// ── Popover ─────────────────────────────────────────────────────────────────
/** A small anchored panel for details (connection info, integrity, help). Closes
 *  on outside click, Escape, scroll or resize. */
export function PopoverPanel({
  anchor,
  onClose,
  children,
  width = 260,
  align = 'start',
  trigger,
}: {
  anchor: DOMRect
  onClose: () => void
  children: ReactNode
  width?: number
  align?: 'start' | 'end' | 'center'
  trigger?: HTMLElement | null
}) {
  const ref = useRef<HTMLDivElement>(null)
  const [pos, setPos] = useState<{ left: number; top: number } | null>(null)
  useLayoutEffect(() => {
    const el = ref.current
    if (!el) return
    const w = el.offsetWidth
    const h = el.offsetHeight
    let top = anchor.bottom + 6
    if (top + h > window.innerHeight - 8) top = Math.max(8, anchor.top - h - 6)
    let left = align === 'end' ? anchor.right - w : align === 'center' ? anchor.left + anchor.width / 2 - w / 2 : anchor.left
    left = Math.min(Math.max(8, left), window.innerWidth - w - 8)
    setPos({ left, top })
  }, [anchor, align])
  useEffect(() => {
    const onDown = (e: MouseEvent) => {
      const t = e.target as Node
      if (!ref.current?.contains(t) && !trigger?.contains(t)) onClose()
    }
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape') { e.preventDefault(); e.stopPropagation(); onClose() } }
    const onScroll = (e: Event) => { if (!ref.current?.contains(e.target as Node)) onClose() }
    window.addEventListener('mousedown', onDown, true)
    window.addEventListener('keydown', onKey, true)
    window.addEventListener('resize', onClose)
    window.addEventListener('scroll', onScroll, true)
    return () => {
      window.removeEventListener('mousedown', onDown, true)
      window.removeEventListener('keydown', onKey, true)
      window.removeEventListener('resize', onClose)
      window.removeEventListener('scroll', onScroll, true)
    }
  }, [onClose, trigger])
  return createPortal(
    <div ref={ref} className="popover-panel-ui" role="dialog" style={{ width, ...(pos ?? { left: -9999, top: -9999 }) }}>
      {children}
    </div>,
    document.body,
  )
}

/** A button that toggles a PopoverPanel. */
export function InfoButton({
  label,
  icon,
  children,
  width,
  align,
  size = 'sm',
  className,
}: {
  label: string
  icon: ReactNode
  children: ReactNode
  width?: number
  align?: 'start' | 'end' | 'center'
  size?: 'sm' | 'md'
  className?: string
}) {
  const [anchor, setAnchor] = useState<DOMRect | null>(null)
  const [trigger, setTrigger] = useState<HTMLElement | null>(null)
  const close = useCallback(() => setAnchor(null), [])
  return (
    <>
      <IconButton
        label={label}
        size={size}
        className={className}
        aria-haspopup="dialog"
        aria-expanded={!!anchor}
        onClick={(e) => { e.stopPropagation(); setTrigger(e.currentTarget); setAnchor(anchor ? null : e.currentTarget.getBoundingClientRect()) }}
      >
        {icon}
      </IconButton>
      {anchor && <PopoverPanel anchor={anchor} onClose={close} width={width} align={align} trigger={trigger}>{children}</PopoverPanel>}
    </>
  )
}
