import { Button, Row, Section, Sheet, TextField } from '../mobile/kit'
import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import jsQR from 'jsqr'
import { fileSrc } from '../lib/api'
import { CameraOff, ClipboardPaste, ImageUp, ScanLine, X } from 'lucide-react'
import { IS_LINUX, IS_MAC, MOBILE_UI } from '../lib/platform'
import { useEscape } from './Dialog'
import { decodeQrFromImage, imageFromTransfer, setScannerDrop } from '../lib/qrImage'


const IMAGE_EXT = /\.(png|jpe?g|gif|webp|heic|heif|bmp|tiff?)$/i

/** Camera QR scanner with camera-less fallbacks: pick / drop / paste a
 *  screenshot of the QR, or paste the code as text. `validate` returns an error
 *  message for a value this field can't use (shown inline; the camera keeps
 *  scanning) or null to accept it. */
export function QrScanner({ onResult, onClose, hint, title = 'Scan QR code', validate }: {
  onResult: (text: string) => void
  onClose: () => void
  hint: string
  title?: string
  validate?: (text: string) => string | null
}) {
  const video = useRef<HTMLVideoElement>(null)
  const canvas = useRef<HTMLCanvasElement>(null)
  const fileInput = useRef<HTMLInputElement>(null)
  const stream = useRef<MediaStream | null>(null)
  const done = useRef(false)
  const cameraEnabled = useRef(true)
  const rejected = useRef('')
  const result = useRef(onResult)
  const check = useRef(validate)
  const closeRef = useRef(onClose)
  useEffect(() => { result.current = onResult; check.current = validate; closeRef.current = onClose }, [onResult, validate, onClose])
  const [error, setError] = useState('')
  const [notice, setNotice] = useState('')
  const [live, setLive] = useState(false)
  const [mirror, setMirror] = useState(false)
  const [paste, setPaste] = useState(false)
  const [reading, setReading] = useState(false)
  const [dropping, setDropping] = useState(false)
  const [code, setCode] = useState('')
  const stop = useCallback(() => { stream.current?.getTracks().forEach(track => track.stop()); stream.current = null; setLive(false) }, [])
  /** true = accepted and closing. */
  const finish = useCallback((text: string): boolean => {
    const value = text.trim()
    if (done.current || !value) return false
    const problem = check.current?.(value) ?? null
    if (problem) {
      // The camera sees the same wrong QR ~10×/s — say it once, keep scanning.
      if (rejected.current !== value) { rejected.current = value; setNotice(problem) }
      return false
    }
    done.current = true
    stop()
    result.current(value)
    return true
  }, [stop])
  const close = useCallback(() => { done.current = true; stop(); closeRef.current() }, [stop])
  const readImage = useCallback(async (file: Blob | null) => {
    if (!file || done.current) return
    setReading(true); setNotice('')
    try {
      const text = await decodeQrFromImage(file)
      if (!text) { setNotice('No QR code found in that image. Try a sharper or closer screenshot.'); return }
      rejected.current = ''
      finish(text)
    } catch {
      setNotice('That image couldn’t be read. Try a PNG or JPEG screenshot.')
    } finally {
      setReading(false)
    }
  }, [finish])

  useEffect(() => {
    let alive = true
    done.current = false
    const fail = (e: unknown) => {
      if (!alive) return
      stop()
      const name = e instanceof Error ? e.name : ''
      const alt = MOBILE_UI ? 'or paste the code.' : 'or scan a screenshot / paste the code below.'
      setError(name === 'NotAllowedError' || name === 'SecurityError'
        ? `Camera access is off for DropBeam. Turn it on in ${IS_MAC ? 'System Settings → Privacy & Security → Camera' : 'your system’s camera privacy settings'}, ${alt}`
        : name === 'NotFoundError' || name === 'OverconstrainedError' ? `No camera found — ${alt}`
        : name === 'NotReadableError' ? `The camera is busy in another app. Close it, ${alt}`
        : `The camera isn’t available here — ${alt}`)
      setPaste(true)
    }
    if (!navigator.mediaDevices?.getUserMedia) fail(new Error('Unavailable'))
    else void navigator.mediaDevices.getUserMedia({ video: { facingMode: { ideal: 'environment' }, width: { ideal: 1280 } }, audio: false }).then(async media => {
      if (!alive || done.current || !cameraEnabled.current) { media.getTracks().forEach(track => track.stop()); return }
      stream.current = media
      // Laptop webcams face the user: mirror the PREVIEW (decoding reads raw frames).
      setMirror(media.getVideoTracks()[0]?.getSettings().facingMode !== 'environment')
      if (video.current) { video.current.srcObject = media; await video.current.play() }
      if (alive) setLive(true)
    }).catch(fail)
    let frame = 0
    const timer = window.setInterval(() => {
      if (done.current || !stream.current) return
      const v = video.current, c = canvas.current
      if (!v || !c || v.readyState < 2 || !v.videoWidth) return
      const scale = Math.min(1, 960 / v.videoWidth)
      c.width = Math.round(v.videoWidth * scale); c.height = Math.round(v.videoHeight * scale)
      const ctx = c.getContext('2d', { willReadFrequently: true })
      if (!ctx) return
      try {
        ctx.drawImage(v, 0, 0, c.width, c.height)
        const data = ctx.getImageData(0, 0, c.width, c.height)
        // Every 4th frame also tries inverted colors (a phone showing a light-on-dark QR).
        const qr = jsQR(data.data, c.width, c.height, { inversionAttempts: ++frame % 4 === 0 ? 'attemptBoth' : 'dontInvert' })
        if (qr?.data) finish(qr.data)
      } catch (e) { fail(e) }
    }, 110)
    return () => { alive = false; clearInterval(timer); stop() }
  }, [finish, stop])

  // ⌘V anywhere in the scanner: a copied screenshot is decoded, copied text is
  // used as the code (unless the user is typing in the paste field).
  useEffect(() => {
    if (MOBILE_UI) return
    const onPaste = (e: ClipboardEvent) => {
      const img = imageFromTransfer(e.clipboardData)
      if (img) { e.preventDefault(); void readImage(img); return }
      if ((e.target as HTMLElement | null)?.closest?.('.qrs-paste')) return
      const text = e.clipboardData?.getData('text/plain') ?? ''
      if (text.trim()) { e.preventDefault(); rejected.current = ''; if (!finish(text)) { setPaste(true); setCode(text.trim()) } }
    }
    document.addEventListener('paste', onPaste)
    const onDrop = (paths: string[]) => {
      const path = paths.find(p => IMAGE_EXT.test(p))
      if (!path) { setNotice('Drop an image (PNG or JPEG) that shows the QR code.'); return }
      void fetch(fileSrc(path)).then(r => r.blob()).then(readImage).catch(() => setNotice('That image couldn’t be opened. Try “Scan from image…”.'))
    }
    const release = setScannerDrop(onDrop)
    return () => {
      document.removeEventListener('paste', onPaste)
      release()
    }
  }, [finish, readImage])

  if (MOBILE_UI) return <Sheet title="Scan QR Code" size="large" onClose={close} primary={paste ? <Button disabled={!code.trim()} onClick={() => finish(code)}>Done</Button> : undefined}>
    <div className="mk-camera"><video ref={video} autoPlay muted playsInline /></div><canvas ref={canvas} hidden />
    <Section footer={hint}><Row title="Paste a Code Instead" tint onPress={() => { cameraEnabled.current = false; stop(); setPaste(true) }} />{paste && <form onSubmit={e => { e.preventDefault(); finish(code) }}><TextField label="Paste a code" placeholder="Code" autoFocus value={code} onChange={e => setCode(e.target.value)} /></form>}</Section>
    {(notice || error) && <Section footer={<span className="mk-error" role="alert">{notice || error}</span>} />}
  </Sheet>

  return createPortal(<ScannerFrame close={close}>
    <div className="card dialog qrs" role="dialog" aria-modal="true" aria-label={title} onClick={e => e.stopPropagation()}
      onDragOver={e => { if (Array.from(e.dataTransfer.items).some(i => i.kind === 'file')) { e.preventDefault(); setDropping(true) } }}
      onDragLeave={e => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropping(false) }}
      onDrop={e => { e.preventDefault(); setDropping(false); const img = imageFromTransfer(e.dataTransfer); if (img) void readImage(img); else setNotice('Drop an image (PNG or JPEG) that shows the QR code.') }}>
      <div className="dialog-head">
        <div style={{ display: 'flex', alignItems: 'center', gap: 11, minWidth: 0 }}>
          <span className="dialog-icon"><ScanLine size={18} /></span>
          <h2 className="dialog-title">{title}</h2>
        </div>
        <button className="icon-btn" aria-label="Close" title="Close (Esc)" onClick={close}><X size={17} /></button>
      </div>
      <div className="dialog-body qrs-body">
        <div className={`qrs-stage${dropping ? ' dropping' : ''}`}>
          <video ref={video} autoPlay muted playsInline className="qrs-video" style={{ transform: mirror ? 'scaleX(-1)' : undefined, opacity: live ? 1 : 0 }} />
          <canvas ref={canvas} hidden />
          {live && <div className="qrs-finder" aria-hidden="true"><i /><i /><i /><i /></div>}
          {!live && !error && <div className="qrs-stage-msg">Starting camera…</div>}
          {error && !dropping && !reading && <div className="qrs-stage-msg"><CameraOff size={26} strokeWidth={1.6} /><span>{error}</span></div>}
          {dropping && <div className="qrs-stage-msg qrs-drop"><ImageUp size={26} strokeWidth={1.6} /><span>Drop the image to scan it</span></div>}
          {reading && <div className="qrs-stage-msg qrs-drop"><span>Reading image…</span></div>}
        </div>
        <p className="qrs-hint">{hint}</p>
        {notice && <p className="qrs-notice" role="alert">{notice}</p>}
        <div className="qrs-alt">
          <button className="btn btn-ghost btn-sm" type="button" disabled={reading} onClick={() => fileInput.current?.click()}><ImageUp size={14} />Scan from image…</button>
          <button className="btn btn-ghost btn-sm" type="button" onClick={() => setPaste(p => !p)}><ClipboardPaste size={14} />Paste the code</button>
          <input ref={fileInput} type="file" accept="image/*" hidden onChange={e => { const f = e.target.files?.[0] ?? null; e.target.value = ''; void readImage(f) }} />
        </div>
        {paste && <form className="qrs-paste" onSubmit={e => { e.preventDefault(); e.stopPropagation(); rejected.current = ''; finish(code) }}>
          <input className="input" aria-label="Paste a code" placeholder="Paste the code…" autoFocus autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" value={code} onChange={e => { setCode(e.target.value); setNotice('') }} />
          <button className="btn btn-primary" disabled={!code.trim()}>Use code</button>
        </form>}
        <p className="qrs-tip">No camera? Drop a screenshot of the QR here, or copy one ({IS_MAC ? '⇧⌘⌃4' : IS_LINUX ? 'Ctrl+Shift+PrtSc' : 'Win+Shift+S'}) and press {IS_MAC ? '⌘V' : 'Ctrl+V'}.</p>
      </div>
    </div>
  </ScannerFrame>, document.body)
}

/** Backdrop for the desktop scanner: click outside or Esc closes it (it stacks
 *  above any dialog that opened it, so Esc only closes the scanner). */
function ScannerFrame({ close, children }: { close: () => void; children: React.ReactNode }) {
  useEscape(close)
  return <div className="dialog-overlay qr-overlay" onMouseDown={e => { if (e.target === e.currentTarget) close() }}>{children}</div>
}
