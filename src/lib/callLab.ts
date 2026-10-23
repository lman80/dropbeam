// Test-only call media (debug builds with DROPBEAM_CALL_FAKE_MEDIA=1): a
// synthetic camera and microphone that need no permission prompt, carrying the
// sender's clock so the receiver can measure real end-to-end delay:
//  - video: the top row of 32 blocks is the sender's wall clock (ms, low 32 bits);
//    the receiver reads it back from the decoded frame → glass-to-glass delay;
//  - audio: a 60 ms 1 kHz beep starts on every whole second of the wall clock;
//    the receiver timestamps each onset → mouth-to-ear delay (minus the sound
//    card's own buffers, which a synthetic source doesn't have).
// Exact on one machine (same clock); across machines it includes clock offset.

const BLOCK = 40
const BITS = 32

export function fakeMedia(video: boolean): MediaStream {
  const ctx = new AudioContext({ sampleRate: 48000 })
  const dest = ctx.createMediaStreamDestination()
  const schedule = () => {
    const wall = Date.now(), t0 = ctx.currentTime
    for (let k = 1; k <= 3; k++) {
      const boundary = Math.ceil(wall / 1000) * 1000 + (k - 1) * 1000
      const at = t0 + (boundary - wall) / 1000
      const o = ctx.createOscillator(), g = ctx.createGain()
      o.frequency.value = 1000
      g.gain.setValueAtTime(0, at)
      g.gain.linearRampToValueAtTime(0.5, at + 0.002)
      g.gain.setValueAtTime(0.5, at + 0.058)
      g.gain.linearRampToValueAtTime(0, at + 0.06)
      o.connect(g).connect(dest)
      o.start(at)
      o.stop(at + 0.07)
    }
  }
  schedule()
  const audioTimer = window.setInterval(schedule, 3000)
  const tracks = [...dest.stream.getAudioTracks()]
  if (video) {
    const c = document.createElement('canvas')
    c.width = 1280; c.height = 720
    const g = c.getContext('2d')!
    let frame = 0
    const draw = () => {
      frame++
      const hue = (frame * 2) % 360
      g.fillStyle = `hsl(${hue} 50% 30%)`
      g.fillRect(0, 0, c.width, c.height)
      g.fillStyle = `hsl(${(hue + 180) % 360} 60% 55%)`
      g.beginPath(); g.arc(640 + 300 * Math.sin(frame / 30), 400, 90, 0, Math.PI * 2); g.fill()
      const now = Date.now() >>> 0
      for (let i = 0; i < BITS; i++) {
        g.fillStyle = (now >>> (BITS - 1 - i)) & 1 ? '#fff' : '#000'
        g.fillRect(i * BLOCK, 0, BLOCK, BLOCK)
      }
      g.fillStyle = '#fff'; g.font = '28px sans-serif'; g.fillText(`DropBeam call test ${new Date().toISOString().slice(11, 23)}`, 24, 700)
    }
    const videoTimer = window.setInterval(draw, 1000 / 30)
    const vt = c.captureStream(30).getVideoTracks()[0]
    vt.addEventListener('ended', () => window.clearInterval(videoTimer))
    tracks.push(vt)
  }
  const stream = new MediaStream(tracks)
  tracks[0].addEventListener('ended', () => { window.clearInterval(audioTimer); void ctx.close() })
  return stream
}

const median = (xs: number[]) => { if (!xs.length) return null; const s = [...xs].sort((a, b) => a - b); return s[Math.floor(s.length / 2)] }

/** Measure delay on the received stream; calls `report` every 5 s. Returns stop. */
export function probe(remote: MediaStream, report: (m: { a2eMs: number | null; g2gMs: number | null; beeps: number; frames: number }) => void): () => void {
  const a2e: number[] = [], g2g: number[] = []
  const ctx = new AudioContext({ sampleRate: 48000 })
  let stopped = false
  const src = remote.getAudioTracks().length ? ctx.createMediaStreamSource(remote) : null
  const proc = ctx.createScriptProcessor(256, 1, 1)
  let quiet = 0
  proc.onaudioprocess = (e) => {
    const d = e.inputBuffer.getChannelData(0)
    let sum = 0
    for (let i = 0; i < d.length; i++) sum += d[i] * d[i]
    const rms = Math.sqrt(sum / d.length)
    if (rms > 0.05 && quiet > 20) {
      // The block that crossed the threshold ended about now; its start is
      // ~256 samples earlier.
      const at = Date.now() - (256 / 48000) * 1000
      const sent = Math.round(at / 1000) * 1000
      const late = at - sent
      if (late >= 0 && late < 900) a2e.push(late)
      quiet = 0
    } else if (rms < 0.01) quiet++
  }
  if (src) { src.connect(proc); proc.connect(ctx.destination) }
  const v = document.createElement('video')
  v.muted = true; v.playsInline = true; v.srcObject = remote
  void v.play().catch(() => {})
  const c = document.createElement('canvas')
  c.width = BITS * BLOCK; c.height = BLOCK
  const g = c.getContext('2d', { willReadFrequently: true })!
  const videoTimer = window.setInterval(() => {
    if (!v.videoWidth) return
    const scale = v.videoWidth / 1280
    g.drawImage(v, 0, 0, BITS * BLOCK * scale, BLOCK * scale, 0, 0, BITS * BLOCK, BLOCK)
    const px = g.getImageData(0, 0, BITS * BLOCK, BLOCK).data
    let n = 0
    for (let i = 0; i < BITS; i++) {
      const x = i * BLOCK + BLOCK / 2, y = BLOCK / 2
      const o = (y * BITS * BLOCK + x) * 4
      n = (n * 2) + (px[o] + px[o + 1] + px[o + 2] > 384 ? 1 : 0)
    }
    const late = ((Date.now() >>> 0) - (n >>> 0)) >>> 0
    if (late < 5000) g2g.push(late)
  }, 100)
  const reportTimer = window.setInterval(() => {
    report({ a2eMs: median(a2e), g2gMs: median(g2g), beeps: a2e.length, frames: g2g.length })
    a2e.length = 0; g2g.length = 0
  }, 5000)
  return () => {
    if (stopped) return
    stopped = true
    window.clearInterval(videoTimer); window.clearInterval(reportTimer)
    proc.disconnect(); src?.disconnect(); void ctx.close()
  }
}
