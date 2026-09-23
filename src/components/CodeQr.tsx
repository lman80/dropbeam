// The one reusable pair behind the rule "anywhere a code can be sent or
// received there's a QR generator and a QR scanner":
//   <QrCodeView value=… />   — scannable QR tile (always dark-on-white, quiet
//                              zone, sized for a phone camera, click to enlarge)
//   <ShareCode code=… />      — QR + the text code + Copy, for every SHOWN code
//   <ScanCodeButton onCode=… />— opens the camera scanner (with screenshot / paste
//                              fallbacks) for every ENTERED code; values are
//                              normalized exactly like pasted ones (lib/codes).
import { useState, type ReactNode } from 'react'
import { createPortal } from 'react-dom'
import { QRCodeSVG } from 'qrcode.react'
import { Check, Copy, Maximize2, QrCode, Smartphone, X } from 'lucide-react'
import { CODE_LABEL, parseCode, qrSpec, wrongCodeMessage, type CodeKind, type ParsedCode } from '../lib/codes'
import { QrScanner } from './QrScanner'
import { useEscape } from './Dialog'
import { useStore } from '../store'

const FG = '#0b0c12'
const BG = '#ffffff'
export const QR_HINT = 'Scan with DropBeam on your phone'

export function QrCodeView({ value, size: base = 200, hint = QR_HINT, label, enlarge = true }: {
  value: string
  /** Base edge in px; long codes grow from here (see qrSpec). */
  size?: number
  hint?: ReactNode | null
  /** Accessible name, e.g. "QR code for your friend code". */
  label?: string
  enlarge?: boolean
}) {
  const [big, setBig] = useState(false)
  const { level, size } = qrSpec(value, base)
  const kind = parseCode(value)?.kind
  const name = label ?? `QR code for ${kind ? CODE_LABEL[kind].replace(/^an? /, 'this ') : 'this code'}`
  const svg = <QRCodeSVG value={value} size={size} level={level} marginSize={2} fgColor={FG} bgColor={BG} title={name} />
  return (
    <figure className="qr-code">
      {enlarge ? (
        <button type="button" className="qr-code-tile" onClick={() => setBig(true)} title="Show larger" aria-label={`${name} — show larger`}>
          {svg}
          <span className="qr-code-zoom" aria-hidden="true"><Maximize2 size={12} /></span>
        </button>
      ) : <div className="qr-code-tile">{svg}</div>}
      {hint && <figcaption className="qr-code-hint"><Smartphone size={13} />{hint}</figcaption>}
      {big && <QrEnlarged value={value} level={level} name={name} onClose={() => setBig(false)} />}
    </figure>
  )
}

function QrEnlarged({ value, level, name, onClose }: { value: string; level: 'L' | 'M'; name: string; onClose: () => void }) {
  useEscape(onClose)
  return createPortal(
    <div className="dialog-overlay qr-overlay qr-enlarged-overlay" onClick={onClose}>
      <div className="qr-enlarged" role="dialog" aria-modal="true" aria-label={name} onClick={(e) => e.stopPropagation()}>
        <div className="qr-enlarged-tile"><QRCodeSVG value={value} size={480} level={level} marginSize={3} fgColor={FG} bgColor={BG} title={name} /></div>
        <p>{QR_HINT}</p>
        <button className="btn btn-ghost" autoFocus onClick={onClose}><X size={15} />Done</button>
      </div>
    </div>,
    document.body,
  )
}

/** QR + text code + Copy. `layout="row"` puts the QR beside the text (wide
 *  cards); "stack" puts it on top (dialogs, narrow panes). */
export function ShareCode({ code, instructions, footer, layout = 'row', size = 200, copyLabel = 'Copy code', hint = QR_HINT }: {
  code: string
  instructions?: ReactNode
  footer?: ReactNode
  layout?: 'row' | 'stack'
  size?: number
  copyLabel?: string
  hint?: ReactNode | null
}) {
  const toast = useStore((s) => s.toast)
  const [copied, setCopied] = useState(false)
  const copy = async () => {
    try {
      await navigator.clipboard.writeText(code)
      setCopied(true)
      setTimeout(() => setCopied(false), 1600)
    } catch {
      toast('error', 'Could not copy — select the code and copy it instead.')
    }
  }
  return (
    <div className={`share-code share-code-${layout}`}>
      <QrCodeView value={code} size={size} hint={hint} />
      <div className="share-code-side">
        {instructions && <div className="share-code-instructions">{instructions}</div>}
        <code className="share-code-text selectable" aria-label="Code">{code}</code>
        <button type="button" className={`btn ${copied ? 'btn-ghost' : 'btn-primary'}`} onClick={copy}>
          {copied ? <Check size={15} /> : <Copy size={15} />}
          {copied ? 'Copied' : copyLabel}
        </button>
        {footer}
      </div>
    </div>
  )
}

/** "Scan QR code" for any field that takes a code. `accept` = the kinds this
 *  field uses; a different DropBeam code goes to `onOther` (e.g. the universal
 *  router) or, without one, gets a clear "that's X — use it in Y" message and
 *  the scanner keeps looking. `onCode` gets the NORMALIZED code. */
export function ScanCodeButton({ onCode, accept, onOther, hint, title, label = 'Scan QR code', className, small = false, iconOnly = false, disabled, style }: {
  onCode: (code: string, parsed: ParsedCode) => void
  accept: readonly CodeKind[]
  onOther?: (parsed: ParsedCode) => void
  hint: string
  title?: string
  label?: string
  className?: string
  /** Compact (btn-sm) ghost button. */
  small?: boolean
  iconOnly?: boolean
  disabled?: boolean
  style?: React.CSSProperties
}) {
  const [open, setOpen] = useState(false)
  const validate = (text: string) => {
    const p = parseCode(text)
    if (p && (accept.includes(p.kind) || onOther)) return null
    return wrongCodeMessage(accept, p)
  }
  return (
    <>
      <button type="button" className={className ?? (small ? 'btn btn-ghost btn-sm' : 'btn btn-ghost')} disabled={disabled} onClick={() => setOpen(true)} aria-label={label} title={iconOnly ? label : undefined} style={style}>
        <QrCode size={small ? 14 : 15} />{!iconOnly && label}
      </button>
      {open && (
        <QrScanner
          hint={hint}
          title={title}
          validate={validate}
          onClose={() => setOpen(false)}
          onResult={(text) => {
            setOpen(false)
            const p = parseCode(text)
            if (!p) return
            if (accept.includes(p.kind)) onCode(p.code, p)
            else onOther?.(p)
          }}
        />
      )}
    </>
  )
}
