import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import type { Delivery } from '../src/lib/api.ts'
import { deliverySummary, deviceStatus, deliveryTone, multiDevice, deliveryIconKind } from '../src/lib/deliveries.ts'

const mac = (state: Delivery['state'], extra: Partial<Delivery> = {}): Delivery => ({ eid: 'm', label: 'Mac', os: 'macos', kind: 'laptop', state, ...extra })
const phone = (state: Delivery['state'], extra: Partial<Delivery> = {}): Delivery => ({ eid: 'p', label: 'iPhone', os: 'ios', kind: 'phone', state, ...extra })

test('every device has it', () => {
  assert.deepEqual(deliverySummary('Alex Chen', [mac('delivered'), phone('delivered')]), { text: 'Delivered to Alex’s Mac and iPhone', tone: 'done' })
})

test('one delivered, one held on the server reads like the owner described', () => {
  const s = deliverySummary('Alex', [mac('delivered'), phone('held', { via: 'Linux Box' })])
  assert.equal(s.text, 'Delivered to Alex’s Mac · iPhone: waiting (Linux Box is holding it)')
  assert.equal(s.tone, 'pending')
})

test('devices in the same situation share a clause, with the right pronoun', () => {
  assert.equal(deliverySummary('Alex', [mac('waiting'), phone('waiting')]).text, 'Alex’s Mac and iPhone: waiting — sends when they’re online')
  assert.equal(deliverySummary('Alex', [mac('delivered'), phone('waiting')]).text, 'Delivered to Alex’s Mac · iPhone: waiting — sends when it’s online')
})

test('a problem on one device is called out', () => {
  const s = deliverySummary('Alex', [mac('delivered'), phone('failed', { note: 'files_gone' })])
  assert.equal(s.text, 'Delivered to Alex’s Mac · iPhone: couldn’t deliver — the files were moved')
  assert.equal(s.tone, 'problem')
  assert.equal(deliveryTone([mac('declined'), phone('delivered')]), 'problem')
})

test('engine errors never leak into the copy, human notes do', () => {
  assert.equal(deviceStatus(phone('waiting', { note: 'connection lost: timed out' })), 'Waiting · sends when it’s online')
  assert.equal(deviceStatus(phone('waiting', { note: 'Linux Box is full' })), 'Waiting · Linux Box is full')
  assert.equal(deviceStatus(phone('held', { via: 'Linux Box' })), 'Waiting · Linux Box is holding it')
  assert.equal(deviceStatus(mac('uploading', { via: 'Linux Box' })), 'Uploading to Linux Box')
})

test('many devices', () => {
  const ds = ['Mac', 'iPhone', 'iPad', 'PC'].map((label, i) => ({ eid: String(i), label, state: 'delivered' as const }))
  assert.equal(deliverySummary('Sam', ds).text, 'Delivered to all 4 of Sam’s devices')
})

test('only multi-device sends get per-device words; icons', () => {
  assert.equal(multiDevice([mac('delivered')]), false)
  assert.equal(multiDevice(null), false)
  assert.equal(multiDevice([mac('delivered'), phone('held')]), true)
  assert.equal(deliveryIconKind({ os: 'macos' }), 'laptop')
  assert.equal(deliveryIconKind({ os: 'ios', kind: 'tablet' }), 'tablet')
  assert.equal(deliveryIconKind({ os: 'windows', kind: 'desktop' }), 'desktop')
})

test('a person’s devices are found through their shared account and labeled like the engine', async () => {
  const { personDevices } = await import('../src/lib/deliveries.ts')
  const f = (id: string, eid: string, os: string, kind: string, acct: string | null) => ({ id, endpointId: eid, accountPub: acct, deviceOs: os, deviceKind: kind, createdAt: 1 })
  const friends = [f('mac', 'e1', 'macos', 'laptop', 'alex'), f('phone', 'e2', 'ios', 'phone', 'alex'), f('sam', 'e3', 'windows', 'desktop', null), f('mac2', 'e0', 'macos', 'laptop', 'alex')]
  const ds = personDevices(friends, 'mac2', 'me')
  assert.deepEqual(ds.map(d => [d.friendId, d.label]), [['mac', 'Mac 2'], ['phone', 'iPhone'], ['mac2', 'Mac 1']])
  assert.deepEqual(personDevices(friends, 'sam', 'me').map(d => d.label), ['PC'])
})
