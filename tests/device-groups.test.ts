import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { groupDevices } from '../src/lib/deviceIcons.ts'

test('device grouping matches only a known own account and preserves order without mutation', () => {
  const friends = [{ id: 'phone', accountPub: 'mine' }, { id: 'friend', accountPub: 'other' }, { id: 'legacy' }, { id: 'tablet', accountPub: 'mine' }, { id: 'empty', accountPub: '' }, { id: 'null', accountPub: null }]
  const before = structuredClone(friends)
  assert.deepEqual(groupDevices(friends, 'mine'), { myDevices: [friends[0], friends[3]], others: [friends[1], friends[2], friends[4], friends[5]] })
  for (const unknown of [undefined, null, '', 'unmatched']) assert.deepEqual(groupDevices(friends, unknown), { myDevices: [], others: friends })
  assert.deepEqual(groupDevices([], 'mine'), { myDevices: [], others: [] })
  assert.deepEqual(friends, before)
})

test('own devices read "Your Mac"/"Your iPhone" and fall back to names when two would collide', async () => {
  const { ownDeviceLabels, deviceNoun } = await import('../src/lib/deviceIcons.ts')
  assert.equal(deviceNoun('phone', 'ios'), 'iPhone')
  assert.equal(deviceNoun('tablet', 'ios'), 'iPad')
  assert.equal(deviceNoun('laptop', 'macos'), 'Mac')
  assert.equal(deviceNoun('desktop', 'windows'), 'PC')
  assert.equal(deviceNoun('phone', null), 'Phone')
  assert.equal(deviceNoun(undefined, undefined), 'Computer')
  assert.deepEqual(ownDeviceLabels([{ id: 'a', name: "Ashton's iPhone", deviceKind: 'phone', deviceOs: 'ios' }, { id: 'b', name: 'Studio', deviceKind: 'desktop', deviceOs: 'macos' }]),
    { a: 'Your iPhone', b: 'Your Mac' })
  assert.deepEqual(ownDeviceLabels([{ id: 'a', name: 'Work Mac', deviceOs: 'macos' }, { id: 'b', name: 'Home Mac', deviceOs: 'macos' }, { id: 'c', name: 'Phone', deviceKind: 'phone', deviceOs: 'ios' }]),
    { a: 'Work Mac', b: 'Home Mac', c: 'Your iPhone' })
})

test('two own devices of one kind read as their models, then names, then numbers', async () => {
  const { ownDeviceLabels } = await import('../src/lib/deviceIcons.ts')
  const phone = (id: string, deviceModel?: string | null, name = 'iPhone') => ({ id, name, deviceKind: 'phone', deviceOs: 'ios', deviceModel })
  const mac = (id: string, deviceModel?: string | null, name = 'Mac') => ({ id, name, deviceKind: 'laptop', deviceOs: 'macos', deviceModel })
  // Different models: the model tells them apart.
  assert.deepEqual(ownDeviceLabels([phone('a', 'iPhone 15'), phone('b', 'iPhone 12')]), { a: 'Your iPhone 15', b: 'Your iPhone 12' })
  assert.deepEqual(ownDeviceLabels([mac('a', 'MacBook Air', "Ashton's MacBook Air"), mac('b', 'Mac mini', 'Studio')]), { a: 'Your MacBook Air', b: 'Your Mac mini' })
  // Only one of a kind: just the noun, even when the model is known.
  assert.deepEqual(ownDeviceLabels([phone('a', 'iPhone 15'), mac('b', 'MacBook Air')]), { a: 'Your iPhone', b: 'Your Mac' })
  // Same model (or an older build that doesn't say): numbered, Finder-style, in a stable order.
  assert.deepEqual(ownDeviceLabels([phone('b', 'iPhone 15'), phone('a', 'iPhone 15')]), { a: 'Your iPhone 15', b: 'Your iPhone 15 (2)' })
  assert.deepEqual(ownDeviceLabels([phone('b'), phone('a', null)]), { a: 'Your iPhone', b: 'Your iPhone (2)' })
  // One knows its model, the other is an older build: still distinct.
  assert.deepEqual(ownDeviceLabels([phone('a', 'iPhone 15'), phone('b')]), { a: 'Your iPhone 15', b: 'Your iPhone' })
  // Same model but telling device names: the names.
  assert.deepEqual(ownDeviceLabels([mac('a', 'MacBook Pro', 'Work Mac'), mac('b', 'MacBook Pro', 'Home Mac')]), { a: 'Work Mac', b: 'Home Mac' })
  // Three iPhones, two the same model.
  assert.deepEqual(ownDeviceLabels([phone('a', 'iPhone 15'), phone('b', 'iPhone 15'), phone('c', 'iPhone 12')]),
    { a: 'Your iPhone 15', b: 'Your iPhone 15 (2)', c: 'Your iPhone 12' })
})

test("a friend's extra devices fold under one deterministic record; own devices never do", async () => {
  const { personGroups } = await import('../src/lib/deviceIcons.ts')
  const f = [
    { id: 'mac', createdAt: 1, accountPub: 'ashton', endpointId: 'e1' },
    { id: 'phone', createdAt: 5, accountPub: 'ashton', endpointId: 'e2' },
    { id: 'mine1', createdAt: 0, accountPub: 'me', endpointId: 'e3' },
    { id: 'mine2', createdAt: 9, accountPub: 'me', endpointId: 'e4' },
    { id: 'solo', createdAt: 2, accountPub: null, endpointId: 'e5' },
  ]
  assert.deepEqual(personGroups(f, 'me'), { phone: 'mac' })
  assert.deepEqual(personGroups(f, null), { phone: 'mac', mine2: 'mine1' })
  // Owner = smallest endpoint id, whatever the local creation order.
  assert.deepEqual(personGroups([{ id: 'x', createdAt: 1, accountPub: 'a', endpointId: 'zz' }, { id: 'y', createdAt: 9, accountPub: 'a', endpointId: 'aa' }], null), { x: 'y' })
})
