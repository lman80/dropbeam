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

test("a friend's extra devices fold under their oldest record; own devices never do", async () => {
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
})
