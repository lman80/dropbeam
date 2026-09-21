import { useRef, useState, type ReactNode } from 'react'
import { ChevronLeft } from 'lucide-react'
import { Button } from './controls'

export type BackAction = { title: string; onPress: () => void }
export interface ScreenProps { title: string; children: ReactNode; leading?: ReactNode | BackAction; trailing?: ReactNode }
export function NavBar({ title, leading, trailing, collapsed = true }: Omit<ScreenProps, 'children'> & { collapsed?: boolean }) {
  const back = leading && typeof leading === 'object' && 'onPress' in leading ? leading as BackAction : null
  return <header className={`mk-navbar${collapsed ? ' is-compact' : ''}`}>
    <div className="mk-bar-leading">{back ? <Button onClick={back.onPress} aria-label={`Back to ${back.title}`}><ChevronLeft size={24} /><span>{back.title}</span></Button> : leading as ReactNode}</div>
    <span className="mk-bar-title" aria-hidden={!collapsed}>{collapsed ? title : ''}</span>
    <div className="mk-bar-trailing">{trailing}</div>
  </header>
}
/** Owns scrolling. Put list content directly inside; do not add a second scroll pane. */
export function Screen({ title, leading, trailing, children }: ScreenProps) {
  const [collapsed, setCollapsed] = useState(false)
  const heading = useRef<HTMLHeadingElement>(null)
  return <div className="mk-screen">
    <div className="mk-scroll" onScroll={event => {
      const scroll = event.currentTarget
      const barHeight = (scroll.firstElementChild as HTMLElement).offsetHeight
      setCollapsed((heading.current?.getBoundingClientRect().bottom ?? Infinity) <= scroll.getBoundingClientRect().top + barHeight)
    }}>
      <NavBar title={title} leading={leading} trailing={trailing} collapsed={collapsed} />
      <h1 ref={heading} className="mk-large-title">{title}</h1>
      {children}
    </div>
  </div>
}
