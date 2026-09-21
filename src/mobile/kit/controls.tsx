import { useId, useState, type ButtonHTMLAttributes, type InputHTMLAttributes, type ReactNode } from 'react'
import { Check, ChevronRight, Search } from 'lucide-react'

export function Button({ filled, destructive, className = '', ...props }: ButtonHTMLAttributes<HTMLButtonElement> & { filled?: boolean; destructive?: boolean }) {
  return <button type="button" {...props} className={`mk-button${filled ? ' mk-filled' : ''}${destructive ? ' mk-destructive' : ''} ${className}`} />
}
export function Switch({ checked, onChange, label, disabled }: { checked: boolean; onChange: (value: boolean) => void; label: string; disabled?: boolean }) {
  return <button type="button" className="mk-switch" role="switch" aria-checked={checked} aria-label={label} disabled={disabled} onClick={() => onChange(!checked)}><span><i /></span></button>
}
export function List({ children }: { children: ReactNode }) { return <div className="mk-list">{children}</div> }
export function Section({ title, footer, children }: { title?: string; footer?: ReactNode; children?: ReactNode }) {
  const id = useId()
  return <section className="mk-section" aria-labelledby={title ? id : undefined}>{title && <h2 id={id} className="mk-section-title">{title}</h2>}{children && <List>{children}</List>}{footer && <div className="mk-section-footer">{footer}</div>}</section>
}
export function IconSquare({ children, color = 'var(--mk-blue)' }: { children: ReactNode; color?: string }) {
  return <span className="mk-icon-square" style={{ backgroundColor: color }} aria-hidden="true">{children}</span>
}
export interface RowProps {
  title: string; subtitle?: ReactNode; value?: ReactNode; icon?: ReactNode; avatar?: ReactNode
  accessory?: 'chevron' | 'toggle' | 'checkmark' | 'none'; checked?: boolean; onChange?: (checked: boolean) => void
  onPress?: () => void; disabled?: boolean; destructive?: boolean; tint?: boolean; emphasized?: boolean; trailing?: ReactNode; children?: ReactNode
  'data-testid'?: string
}
export function Row({ title, subtitle, value, icon, avatar, accessory = 'none', checked = false, onChange, onPress, disabled, destructive, tint, emphasized, trailing, children, 'data-testid': testId }: RowProps) {
  const main = <>{(avatar || icon) && <span className="mk-row-image">{avatar || icon}</span>}<span className="mk-row-text"><span className={`mk-row-title${emphasized ? ' mk-headline' : ''}`}>{title}</span>{subtitle && <span className="mk-row-subtitle">{subtitle}</span>}{children}</span>{value != null && <span className="mk-row-value">{value}</span>}{accessory === 'chevron' && <ChevronRight className="mk-chevron" size={16} />}{accessory === 'checkmark' && checked && <Check className="mk-check" size={21} />}</>
  return <div className={`mk-row${avatar || icon ? ' mk-row-with-image' : ''}${destructive ? ' mk-destructive' : ''}${tint ? ' mk-tint' : ''}`} data-testid={testId}>
    {onPress ? <button type="button" className="mk-row-main" onClick={onPress} disabled={disabled}>{main}</button> : <div className="mk-row-main">{main}</div>}
    {accessory === 'toggle' && <Switch label={title} checked={checked} onChange={onChange || (() => {})} disabled={disabled} />}{trailing && <div className="mk-row-trailing">{trailing}</div>}
  </div>
}
export function ProgressRow({ progress, ...props }: RowProps & { progress: number | null }) {
  return <Row {...props}>{progress != null && <span className="mk-progress" role="progressbar" aria-label={`${props.title} progress`} aria-valuemin={0} aria-valuemax={100} aria-valuenow={Math.round(Math.max(0, Math.min(100, progress)))}><i style={{ width: `${Math.max(0, Math.min(100, progress))}%` }} /></span>}</Row>
}
export function SearchField({ value, onChange, placeholder = 'Search', ...props }: Omit<InputHTMLAttributes<HTMLInputElement>, 'onChange'> & { onChange: (value: string) => void }) {
  return <label className="mk-search"><Search size={18} aria-hidden="true" /><input {...props} type="search" aria-label={props['aria-label'] || placeholder} placeholder={placeholder} value={value} onChange={e => onChange(e.target.value)} /></label>
}
export function TextField({ label, ...props }: InputHTMLAttributes<HTMLInputElement> & { label: string }) {
  return <label className="mk-field">{props.type === 'number' && <span>{label}</span>}<input autoCorrect="off" autoCapitalize="none" spellCheck={false} {...props} aria-label={label} /></label>
}
export function SegmentedControl<T extends string>({ options, value, onChange, label }: { options: readonly { value: T; label: string }[]; value: T; onChange: (value: T) => void; label: string }) {
  return <div className="mk-segmented" role="group" aria-label={label}>{options.map(option => <button type="button" key={option.value} aria-pressed={value === option.value} onClick={() => onChange(option.value)}>{option.label}</button>)}</div>
}
export function Avatar({ name, src, size = 40, badge }: { name: string; src?: string | null; size?: number; badge?: ReactNode }) {
  const [broken, setBroken] = useState<string | null>(null)
  const initials = name.trim().split(/\s+/).slice(0, 2).map(part => [...part][0] || '').join('').toUpperCase() || '?'
  return <span className="mk-avatar" style={{ width: size, height: size, fontSize: size * .36 }} aria-label={name}><span className="mk-avatar-picture">{src && broken !== src ? <img src={src} alt="" onError={() => setBroken(src)} /> : initials}</span>{badge && <span className="mk-device-badge">{badge}</span>}</span>
}
export function Badge({ count }: { count: number }) { return count > 0 ? <span className="mk-badge" aria-label={`${count} unread`}>{count > 99 ? '99+' : count}</span> : null }
export function EmptyState({ icon, title, body }: { icon: ReactNode; title: string; body?: string }) {
  return <div className="mk-empty"><span>{icon}</span><h2>{title}</h2>{body && <p>{body}</p>}</div>
}
