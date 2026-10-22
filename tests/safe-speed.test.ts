import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { applySafeSpeed, safeSpeedResult, safeSpeedStep, STEPS_MBPS } from '../src/lib/safeSpeed.ts'

test('the safe speed test says what it is doing in plain words (#20)', () => {
  assert.equal(safeSpeedStep(null), 'Finding your router…')
  assert.equal(safeSpeedStep({ phase: 'device' }), 'Finding a device to send test data to…')
  assert.equal(safeSpeedStep({ phase: 'testing', rateMbps: 150, health: 'strained', target: 'Ashton’s iPhone' }), 'Trying 150 Mbps to Ashton’s iPhone — your router is slowing down')
  assert.equal(safeSpeedStep({ phase: 'testing', rateMbps: 25, health: 'fine' }), 'Trying 25 Mbps — your router is fine')
  assert.deepEqual(STEPS_MBPS, [...STEPS_MBPS].sort((a, b) => a - b), 'slowest first')
})

test('a result recommends a limit (local network too when that is where it struggled) or none', () => {
  const local = { kind: 'limit', safeMbps: 117, troubleMbps: 198, target: 'Mac', local: true, routerMs: 3 } as const
  assert.deepEqual(safeSpeedResult(local), { title: 'We recommend 117 Mbps', detail: 'Your router started to struggle at about 198 Mbps on your home network. 117 Mbps leaves it room to breathe.', apply: 117 })
  assert.deepEqual(applySafeSpeed(local), { uploadLimitMbps: 117, limitLocal: true })
  assert.deepEqual(applySafeSpeed({ ...local, local: false }), { uploadLimitMbps: 117 }, 'an internet result leaves local sends alone')
  const fine = { kind: 'noLimit', topMbps: 940, target: 'Mac', local: true, routerMs: 2 } as const
  assert.equal(safeSpeedResult(fine).title, 'No limit needed')
  assert.deepEqual(applySafeSpeed(fine), { uploadLimitMbps: 0 })
})
