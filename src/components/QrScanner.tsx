import { Button, Row, Section, Sheet, TextField } from '../mobile/kit'
import { useCallback, useEffect, useRef, useState } from 'react'
import { createPortal } from 'react-dom'
import jsQR from 'jsqr'
import { MOBILE_UI } from '../lib/platform'

export function QrScanner({ onResult, onClose, hint }: { onResult: (text: string) => void; onClose: () => void; hint: string }) {
  const video = useRef<HTMLVideoElement>(null)
  const canvas = useRef<HTMLCanvasElement>(null)
  const stream = useRef<MediaStream | null>(null)
  const done = useRef(false)
  const cameraEnabled = useRef(true)
  const result = useRef(onResult)
  useEffect(() => { result.current = onResult }, [onResult])
  const [error, setError] = useState('')
  const [paste, setPaste] = useState(false)
  const [code, setCode] = useState('')
  const stop = useCallback(() => { stream.current?.getTracks().forEach(track => track.stop()); stream.current = null }, [])
  const finish = useCallback((text: string) => {
    if (done.current || !text.trim()) return
    done.current = true
    stop()
    result.current(text.trim())
  }, [stop])
  const close = () => { done.current = true; stop(); onClose() }
  useEffect(() => {
    let alive = true
    done.current = false
    const fail = (e: unknown) => {
      if (!alive) return
      stop()
      const name = e instanceof Error ? e.name : ''
      setError(name === 'NotAllowedError' || name === 'SecurityError'
        ? 'Camera access was denied. Allow it in System Settings > Privacy & Security > Camera, or paste the code.'
        : name === 'NotFoundError' ? 'No camera was found. Connect a camera, or paste the code.'
        : name === 'NotReadableError' ? 'The camera is busy or unavailable. Close other camera apps, or paste the code.'
        : 'The camera is unavailable in this app. Paste the code instead.')
      setPaste(true)
    }
    if (!navigator.mediaDevices?.getUserMedia) fail(new Error('Unavailable'))
    else void navigator.mediaDevices.getUserMedia({ video: { facingMode: { ideal: 'environment' } }, audio: false }).then(async media => {
      if (!alive || done.current || !cameraEnabled.current) { media.getTracks().forEach(track => track.stop()); return }
      stream.current = media
      if (video.current) { video.current.srcObject = media; await video.current.play() }
    }).catch(fail)
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
        const qr = jsQR(data.data, c.width, c.height, { inversionAttempts: 'dontInvert' })
        if (qr?.data) finish(qr.data)
      } catch (e) { fail(e) }
    }, 100)
    return () => { alive = false; clearInterval(timer); stop() }
  }, [finish, stop])
  if (MOBILE_UI) return <Sheet title="Scan QR Code" size="large" onClose={close} primary={paste ? <Button disabled={!code.trim()} onClick={() => finish(code)}>Done</Button> : undefined}>
    <div className="mk-camera"><video ref={video} autoPlay muted playsInline /></div><canvas ref={canvas} hidden />
    <Section footer={hint}><Row title="Paste a Code Instead" tint onPress={() => { cameraEnabled.current = false; stop(); setPaste(true) }} />{paste && <form onSubmit={e => { e.preventDefault(); finish(code) }}><TextField label="Paste a code" placeholder="Code" autoFocus value={code} onChange={e => setCode(e.target.value)} /></form>}</Section>
    {error && <Section footer={<span className="mk-error" role="alert">{error}</span>} />}
  </Sheet>
  return createPortal(<div className="dialog-overlay qr-overlay" onKeyDown={e => { if (e.key === 'Escape') { e.stopPropagation(); close() } }}>
    <div className={`${MOBILE_UI ? 'dialog mobile-sheet' : 'card dialog'} qr-scanner`} role="dialog" aria-modal="true" aria-label="Scan QR code">
      <video ref={video} autoPlay muted playsInline className="qr-video" />
      <canvas ref={canvas} hidden />
      <div className="qr-viewfinder" aria-hidden="true" />
      <div className="qr-controls">
        <p>{hint}</p>
        {error && <p role="alert">{error}</p>}
        <button className="btn btn-ghost" onClick={() => setPaste(true)}>Paste a code instead</button>
        {paste && <form onSubmit={e => { e.preventDefault(); finish(code) }}><input className="input" aria-label="Paste a code" autoCapitalize="none" autoCorrect="off" spellCheck={false} value={code} onChange={e => setCode(e.target.value)} /><button className="btn btn-primary" disabled={!code.trim()}>Use code</button></form>}
        <button className="btn btn-ghost" autoFocus onClick={close}>Cancel</button>
      </div>
    </div>
  </div>, document.body)
}
