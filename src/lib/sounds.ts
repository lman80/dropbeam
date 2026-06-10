// Tiny Web Audio sound kit — short, soft, musical cues for transfer events.
// Generated on the fly so there are no audio assets to bundle and they stay
// crisp at any volume. All gentle (sine/triangle, low gain, quick decay).

let ctx: AudioContext | null = null

function audio(): AudioContext | null {
  try {
    if (!ctx) {
      const Ctor =
        window.AudioContext ||
        (window as unknown as { webkitAudioContext?: typeof AudioContext }).webkitAudioContext
      if (!Ctor) return null
      ctx = new Ctor()
    }
    if (ctx.state === 'suspended') void ctx.resume()
    return ctx
  } catch {
    return null
  }
}

/** One enveloped note. `at` is an offset (seconds) from now. */
function note(
  freq: number,
  at: number,
  dur: number,
  peak = 0.13,
  type: OscillatorType = 'sine',
) {
  const c = audio()
  if (!c) return
  const t0 = c.currentTime + at
  const osc = c.createOscillator()
  const gain = c.createGain()
  osc.type = type
  osc.frequency.setValueAtTime(freq, t0)
  osc.connect(gain)
  gain.connect(c.destination)
  // Quick attack, smooth exponential decay — reads as a soft "blip", not a beep.
  gain.gain.setValueAtTime(0.0001, t0)
  gain.gain.exponentialRampToValueAtTime(peak, t0 + 0.012)
  gain.gain.exponentialRampToValueAtTime(0.0001, t0 + dur)
  osc.start(t0)
  osc.stop(t0 + dur + 0.03)
}

/** Sent: a bright, confident two-note rise (D5 → A5). */
export function playSent() {
  note(587.33, 0, 0.16)
  note(880.0, 0.08, 0.22)
}

/** Received / completed: a warm little chime (A5 → D6) with a soft tail. */
export function playReceived() {
  note(880.0, 0, 0.15)
  note(1174.66, 0.1, 0.3, 0.12)
}

/** Incoming auto-receive starting: a soft, low "swoosh in". */
export function playIncoming() {
  note(440.0, 0, 0.22, 0.09, 'triangle')
}

/** Manual-accept offer waiting: a gentle attention "pop-pop". */
export function playOffer() {
  note(523.25, 0, 0.14, 0.12, 'triangle')
  note(659.25, 0.11, 0.2, 0.12, 'triangle')
}

/** Error / canceled: a soft descending two-note "uh-oh" (G4 → D4), not harsh. */
export function playError() {
  note(392.0, 0, 0.18, 0.11, 'triangle')
  note(293.66, 0.12, 0.34, 0.12, 'triangle')
}
