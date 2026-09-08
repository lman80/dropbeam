import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { LandedEta } from '../src/lib/eta.ts'

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
