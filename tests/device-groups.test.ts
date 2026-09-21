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
