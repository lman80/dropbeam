import { Button, Row, Section, Sheet, TextField } from '../mobile/kit'
import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import jsQR from 'jsqr'
import { fileSrc } from '../lib/api'
import { CameraOff, ImageUp, X, ZoomIn, ZoomOut } from 'lucide-react'
import { IS_MAC, MOBILE_UI } from '../lib/platform'
import { useEscape, useModalFocus } from './Dialog'
import { IconButton } from './ui'
import { decodeQrFromImage, imageFromTransfer, setScannerDrop } from '../lib/qrImage'
import { hardwareZoom, MAX_DIGITAL_ZOOM, zoomCrop, zoomsForTick } from '../lib/qrZoom'


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
  // Zoom (#85): the slider's value; `hw` = the camera's own zoom range, if any.
  const [zoom, setZoom] = useState(1)
  const [hw, setHw] = useState<{ min: number; max: number; step: number } | null>(null)
  const zoomRef = useRef(1)
  const hwRef = useRef(false)
  const changeZoom = (z: number) => {
    setZoom(z); zoomRef.current = z
    const track = stream.current?.getVideoTracks()[0]
    if (hw && track) void track.applyConstraints({ advanced: [{ zoom: z } as unknown as MediaTrackConstraintSet] }).catch(() => {})
  }
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
      const where = IS_MAC ? 'System Settings → Privacy & Security → Camera' : 'your system’s camera privacy settings'
      setError(name === 'NotAllowedError' || name === 'SecurityError'
        ? `Camera access is off. Turn it on in ${where}.`
        : name === 'NotFoundError' || name === 'OverconstrainedError' ? 'No camera found.'
        : name === 'NotReadableError' ? 'The camera is in use by another app.'
        : 'The camera isn’t available.')
      setPaste(true)
    }
    if (!navigator.mediaDevices?.getUserMedia) fail(new Error('Unavailable'))
    // As many pixels as the camera gives (up to 1080p): zooming in reads the
    // centre at full resolution, so a far-away code keeps enough detail.
    else void navigator.mediaDevices.getUserMedia({ video: { facingMode: { ideal: 'environment' }, width: { ideal: 1920 }, height: { ideal: 1080 } }, audio: false }).then(async media => {
      if (!alive || done.current || !cameraEnabled.current) { media.getTracks().forEach(track => track.stop()); return }
      stream.current = media
      const range = hardwareZoom(media.getVideoTracks()[0])
      hwRef.current = !!range
      setHw(range)
      if (range) { setZoom(range.min); zoomRef.current = range.min }
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
      const ctx = c.getContext('2d', { willReadFrequently: true })
      if (!ctx) return
      try {
        // Every 4th frame also tries inverted colors (a phone showing a light-on-dark QR).
        const invert = ++frame % 4 === 0 ? 'attemptBoth' : 'dontInvert'
        // The whole frame and a zoomed-in centre (#85); with the camera's own
        // zoom the frame is already zoomed, so the slider's value isn't re-applied.
        for (const z of zoomsForTick(frame, hwRef.current ? 1 : zoomRef.current)) {
          const r = zoomCrop(v.videoWidth, v.videoHeight, z)
          c.width = r.dw; c.height = r.dh
          ctx.drawImage(v, r.sx, r.sy, r.sw, r.sh, 0, 0, r.dw, r.dh)
          const data = ctx.getImageData(0, 0, r.dw, r.dh)
          const qr = jsQR(data.data, r.dw, r.dh, { inversionAttempts: invert })
          if (qr?.data && finish(qr.data)) return
          if (done.current) return
        }
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
      void fetch(fileSrc(path)).then(r => r.blob()).then(readImage).catch(() => setNotice('That image couldn’t be opened. Try “Choose image…”.'))
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

  const status = reading ? 'Reading image…' : dropping ? 'Drop the image to scan it' : error || (!live ? 'Starting camera…' : '')
  return createPortal(<ScannerFrame close={close}>
    <div className="dialog dialog-panel qrs" role="dialog" aria-modal="true" aria-label={title} onClick={e => e.stopPropagation()}
      onDragOver={e => { if (Array.from(e.dataTransfer.items).some(i => i.kind === 'file')) { e.preventDefault(); setDropping(true) } }}
      onDragLeave={e => { if (!e.currentTarget.contains(e.relatedTarget as Node | null)) setDropping(false) }}
      onDrop={e => { e.preventDefault(); setDropping(false); const img = imageFromTransfer(e.dataTransfer); if (img) void readImage(img); else setNotice('Drop an image (PNG or JPEG) that shows the QR code.') }}>
      <div className="dialog-head">
        <h2 className="dialog-title">{title}</h2>
        <IconButton label="Close" tooltip="Close (Esc)" onClick={close}><X /></IconButton>
      </div>
      <div className="dialog-body qrs-body">
        <div className={`qrs-stage${live && !dropping && !reading ? ' live' : ''}${dropping ? ' dropping' : ''}`}>
          <video ref={video} autoPlay muted playsInline className="qrs-video" style={{ transform: `${mirror ? 'scaleX(-1) ' : ''}${!hw && zoom > 1 ? `scale(${zoom})` : ''}`.trim() || undefined, opacity: live ? 1 : 0 }} />
          <canvas ref={canvas} hidden />
          {live && !dropping && !reading && <div className="qrs-finder" aria-hidden="true"><i /><i /><i /><i /></div>}
          {status && <div className="qrs-stage-msg" role={error ? 'alert' : 'status'}>
            {error && !dropping && !reading && <CameraOff size={22} strokeWidth={1.6} aria-hidden />}
            {dropping && <ImageUp size={22} strokeWidth={1.6} aria-hidden />}
            <span>{status}</span>
          </div>}
        </div>
        {live && !dropping && !reading && <label className="qrs-zoom" title="Too far away? Zoom in on the code.">
          <ZoomOut size={15} aria-hidden />
          <input type="range" aria-label="Zoom" min={hw?.min ?? 1} max={hw ? Math.min(hw.max, 10) : MAX_DIGITAL_ZOOM} step={hw?.step ?? 0.1} value={zoom}
            onChange={e => changeZoom(Number(e.target.value))} />
          <ZoomIn size={15} aria-hidden />
        </label>}
        <p className="qrs-hint">{hint}</p>
        {notice && <p className="qrs-notice" role="alert">{notice}</p>}
        {paste ? <form className="qrs-paste" onSubmit={e => { e.preventDefault(); e.stopPropagation(); rejected.current = ''; finish(code) }}>
          <input className="input" aria-label="Paste a code" placeholder="Paste the code" autoFocus autoCapitalize="none" autoCorrect="off" spellCheck={false} autoComplete="off" value={code} onChange={e => { setCode(e.target.value); setNotice('') }} />
          <button className="btn btn-primary" disabled={!code.trim()}>Use Code</button>
        </form> : null}
      </div>
      <div className="dialog-actions dialog-footer qrs-actions">
        <button className="btn btn-plain" type="button" disabled={reading} onClick={() => fileInput.current?.click()}>Choose Image…</button>
        {!paste && <button className="btn btn-plain" type="button" onClick={() => setPaste(true)}>Paste Code</button>}
        <span className="spacer" />
        <button className="btn btn-secondary" type="button" onClick={close}>Cancel</button>
        <input ref={fileInput} type="file" accept="image/*" hidden onChange={e => { const f = e.target.files?.[0] ?? null; e.target.value = ''; void readImage(f) }} />
      </div>
    </div>
  </ScannerFrame>, document.body)
}

/** Backdrop for the desktop scanner: click outside or Esc closes it (it stacks
 *  above any dialog that opened it, so Esc only closes the scanner). */
function ScannerFrame({ close, children }: { close: () => void; children: React.ReactNode }) {
  useEscape(close)
  const ref = useRef<HTMLDivElement>(null)
  useModalFocus(ref)
  return <div ref={ref} className="dialog-overlay qr-overlay" onMouseDown={e => { if (e.target === e.currentTarget) close() }}>{children}</div>
}
