import { useEffect, useRef, useState, type ReactNode } from 'react'

/** Mount only in MOBILE_UI pages. Observes the actual shell scroll container. */
export function MobileHeader({ title, subtitle, actions }: { title: string; subtitle?: string; actions?: ReactNode }) {
  const large = useRef<HTMLHeadingElement>(null)
  const compact = useRef<HTMLDivElement>(null)
  const [collapsed, setCollapsed] = useState(false)
  useEffect(() => {
    const target = large.current
    if (!target) return
    const observer = new IntersectionObserver(([entry]) => {
      setCollapsed(!entry.isIntersecting && entry.boundingClientRect.top < (entry.rootBounds?.top ?? 0))
    }, { root: target.closest('main'), rootMargin: `-${compact.current?.offsetHeight ?? 44}px 0px 0px 0px` })
    observer.observe(target)
    return () => observer.disconnect()
  }, [])
  return <>
    <div ref={compact} className={`mobile-header-compact glass${collapsed ? ' visible' : ''}`} aria-hidden={!collapsed} inert={!collapsed}>
      <span>{title}</span><div className="mobile-header-actions">{collapsed && actions}</div>
    </div>
    <header className="mobile-header">
      <div className="mobile-header-line"><h1 ref={large}>{title}</h1><div className="mobile-header-actions" inert={collapsed}>{actions}</div></div>
      {subtitle && <p className="ios-sub">{subtitle}</p>}
    </header>
  </>
}
