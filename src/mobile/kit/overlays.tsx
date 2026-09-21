import { useEffect, useRef, useState, type CSSProperties, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { Button } from './controls'

let modalCount = 0
let priorOverflow = ''
/** Native dialog supplies focus trapping, background inertness and focus restoration. */
function Modal({ title, onClose, dismissible = true, className, children, style }: { title: string; onClose: () => void; dismissible?: boolean; className: string; children: ReactNode; style?: CSSProperties }) {
  const dialog = useRef<HTMLDialogElement>(null)
  useEffect(() => {
    const previous = document.activeElement as HTMLElement | null
    if (modalCount++ === 0) { priorOverflow = document.body.style.overflow; document.body.style.overflow = 'hidden'; document.documentElement.classList.add('mk-modal-open') }
    dialog.current?.showModal()
    return () => {
      if (--modalCount === 0) { document.body.style.overflow = priorOverflow; document.documentElement.classList.remove('mk-modal-open') }
      if (previous?.isConnected) previous.focus({ preventScroll: true })
    }
  }, [])
  return createPortal(<dialog ref={dialog} className={`mk-modal ${className}`} aria-label={title} onCancel={e => { e.preventDefault(); e.stopPropagation(); if (dismissible) onClose() }} onClick={e => { if (dismissible && e.target === e.currentTarget) onClose() }}><div className="mk-modal-surface" style={style}>{children}</div></dialog>, document.body)
}
export function Sheet({ title, onClose, children, primary, size = 'medium', dismissible = true }: { title: string; onClose: () => void; children: ReactNode; primary?: ReactNode; size?: 'medium' | 'large'; dismissible?: boolean }) {
  return <Modal title={title} onClose={onClose} dismissible={dismissible} className={`mk-sheet mk-sheet-${size}`}><div className="mk-grabber" aria-hidden="true" /><header className="mk-sheet-bar"><div>{dismissible && <Button onClick={onClose}>Cancel</Button>}</div><h2>{title}</h2><div>{primary}</div></header><div className="mk-sheet-content">{children}</div></Modal>
}
export interface MenuAction { label: string; icon?: ReactNode; destructive?: boolean; disabled?: boolean; onPress: () => void }
export function ActionSheet({ title = 'Actions', message, actions, onClose, dismissOnAction = true }: { title?: string; message?: string; actions: MenuAction[]; onClose: () => void; dismissOnAction?: boolean }) {
  return <Modal title={title} onClose={onClose} className="mk-action-sheet"><div className="mk-action-group">{message && <p className="mk-action-message">{message}</p>}{actions.map(action => <Button key={action.label} destructive={action.destructive} disabled={action.disabled} onClick={() => { if (dismissOnAction) onClose(); action.onPress() }}>{action.label}</Button>)}</div><div className="mk-action-group"><Button onClick={onClose}>Cancel</Button></div></Modal>
}
export function Alert({ title, message, children, actions, onClose }: { title: string; message?: string; children?: ReactNode; actions: MenuAction[]; onClose: () => void }) {
  return <Modal title={title} onClose={onClose} className="mk-alert"><div className="mk-alert-body"><h2>{title}</h2>{message && <p>{message}</p>}{children}</div><div className="mk-alert-actions">{actions.map(action => <Button key={action.label} destructive={action.destructive} disabled={action.disabled} onClick={action.onPress}>{action.label}</Button>)}</div></Modal>
}
export function ContextMenu({ children, actions, label = 'Actions' }: { children: ReactNode; actions: MenuAction[]; label?: string }) {
  const [point, setPoint] = useState<{ x: number; y: number } | null>(null)
  const timer = useRef<ReturnType<typeof setTimeout> | undefined>(undefined)
  const origin = useRef({ x: 0, y: 0 })
  const clear = () => clearTimeout(timer.current)
  useEffect(() => clear, [])
  const open = (x: number, y: number) => { clear(); setPoint({ x: Math.max(8, Math.min(x, window.innerWidth - 258)), y: Math.max(8, Math.min(y, window.innerHeight - actions.length * 44 - 24)) }) }
  return <><div className="mk-context-trigger" tabIndex={0} role="button" aria-label={label} aria-haspopup="menu" onPointerDown={e => { origin.current = { x: e.clientX, y: e.clientY }; if (e.pointerType !== 'mouse') timer.current = setTimeout(() => open(e.clientX, e.clientY), 400) }} onPointerMove={e => { if (Math.hypot(e.clientX - origin.current.x, e.clientY - origin.current.y) > 8) clear() }} onPointerUp={clear} onPointerCancel={clear} onClick={e => { if (e.detail === 0 || ((e.nativeEvent as PointerEvent).pointerType === 'mouse' || window.matchMedia('(pointer: fine)').matches)) open(e.clientX || e.currentTarget.getBoundingClientRect().left, e.clientY || e.currentTarget.getBoundingClientRect().bottom) }} onContextMenu={e => { e.preventDefault(); open(e.clientX, e.clientY) }} onKeyDown={e => { if (e.key === 'Enter' || e.key === ' ') { e.preventDefault(); const r = e.currentTarget.getBoundingClientRect(); open(r.left, r.bottom) } }}>{children}</div>{point && <Modal title={label} onClose={() => setPoint(null)} className="mk-context-menu" style={{ left: point.x, top: point.y }}><div role="menu" aria-label={label}>{actions.map(action => <Button key={action.label} role="menuitem" destructive={action.destructive} disabled={action.disabled} onClick={() => { setPoint(null); action.onPress() }}><span>{action.label}</span>{action.icon}</Button>)}</div></Modal>}</>
}
