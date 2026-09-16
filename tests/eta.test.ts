import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { LandedEta, TransferRate, etaAt, sampleRate, trimSamples } from '../src/lib/eta.ts'

test('first landed MiB is a baseline, independent of the startup delay', () => {
  const eta = new LandedEta()
  assert.equal(eta.update(0, 0, 80e6), null)
  assert.equal(eta.update(4000, 1e6, 80e6), null)
  assert.equal(eta.update(4250, 2e6, 80e6), 19.5)
})

test('three-second window forgets ramp-up and handles stalls and retries', () => {
  const eta = new LandedEta()
  eta.update(0, 1e6, 80e6)
  eta.update(1000, 1.1e6, 80e6)
  eta.update(2000, 5.1e6, 80e6)
  eta.update(3000, 9.1e6, 80e6)
  assert.equal(eta.update(4000, 13.1e6, 80e6), (80e6 - 13.1e6) / 4e6)
  assert.equal(eta.update(8000, 13.1e6, 80e6), null)
  assert.equal(eta.update(9000, 1e6, 80e6), null)
  assert.equal(eta.update(9250, 2e6, 80e6), 19.5)
  assert.equal(eta.update(9500, 80e6, 80e6), 0)
})

test('live rate measures the last few seconds, the average the whole run', () => {
  const rate = new TransferRate()
  // Under two samples there is nothing to measure — the card falls back to the
  // engine's own figures instead of showing a made-up number.
  rate.update(0, 0)
  assert.equal(rate.live(), null)
  assert.equal(rate.average(), null)
  assert.equal(rate.count, 1)
  // A slow first second, then 10 MB/s.
  rate.update(1000, 1e6)
  assert.equal(rate.live(), 1e6)
  assert.equal(rate.average(), 1e6)
  for (let s = 2; s <= 7; s++) rate.update(s * 1000, 1e6 + (s - 1) * 10e6)
  assert.equal(rate.live(), 10e6) // last 5s only
  assert.equal(rate.average(), 61e6 / 7) // includes the slow start
  assert.equal(rate.startedAt, 0)
})

test('a stalled transfer reads zero and never blinks between frames', () => {
  const rate = new TransferRate()
  rate.update(0, 0)
  rate.update(1000, 4e6)
  assert.equal(rate.live(), 4e6)
  // Out-of-order and rewound frames are ignored, and hold the last reading.
  rate.update(500, 9e6)
  rate.update(2000, 1e6)
  assert.equal(rate.live(), 4e6)
  // Genuinely stalled for the whole window: measured, and honestly zero.
  for (let s = 2; s <= 8; s++) rate.update(s * 1000, 4e6)
  assert.equal(rate.live(), 0)
  assert.equal(rate.average(), 4e6 / 8)
})

test('sample rate and window trimming are pure', () => {
  assert.equal(sampleRate([]), null)
  assert.equal(sampleRate([{ at: 0, bytes: 0 }]), null)
  assert.equal(sampleRate([{ at: 5, bytes: 0 }, { at: 5, bytes: 9 }]), null)
  assert.equal(sampleRate([{ at: 0, bytes: 1e6 }, { at: 2000, bytes: 9e6 }]), 4e6)
  const samples = [0, 1000, 2000, 6000, 7000].map((at) => ({ at, bytes: at }))
  // Keeps the sample straddling the window start, drops what is wholly behind it.
  assert.deepEqual(trimSamples(samples, 7000, 5000).map((s) => s.at), [2000, 6000, 7000])
  assert.deepEqual(trimSamples(samples, 7000, 20000), samples)
  assert.deepEqual(trimSamples([], 7000, 5000), [])
})

test('time left comes from whichever rate the card is showing', () => {
  assert.equal(etaAt(20e6, 100e6, 10e6), 8)
  assert.equal(etaAt(100e6, 100e6, 10e6), 0)
  assert.equal(etaAt(20e6, 100e6, 0), null)
  assert.equal(etaAt(20e6, 100e6, null), null)
  assert.equal(etaAt(20e6, 0, 10e6), null)
})
