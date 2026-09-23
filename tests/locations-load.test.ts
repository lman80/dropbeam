import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { loadLocations, nativeLocationRows } from '../src/lib/locationsLoad.ts'

const friends = [
  { id: 'linux', name: 'Linux Box', endpointId: 'endpoint' },
  { id: 'mac', name: 'Mac', endpointId: 'other' },
  { id: 'legacy', name: 'Legacy', endpointId: null },
]
const folder = { id: 'nas', name: 'Shared', rights: { upload: true, manage: false } }

test('locations loader probes stale presence and returns the native per-friend shape', async () => {
  const listed: string[] = []
  const result = await loadLocations({ friends, online: () => false, probe: async id => id === 'linux', list: async id => { listed.push(id); return [folder] } })
  assert.deepEqual(listed, ['linux'])
  assert.deepEqual(result, [
    { friendId: 'linux', friendName: 'Linux Box', online: true, locations: [folder], error: null, status: 'ready' },
    { friendId: 'mac', friendName: 'Mac', online: false, locations: [], error: 'Mac is offline', status: 'offline' },
  ])
})
test('one peer failing preserves its cache without hiding another peer or waiting to publish', async () => {
  const published: string[] = []
  const result = await loadLocations({ friends, online: () => true, cached: { linux: [folder] }, list: async id => {
    if (id === 'linux') throw new Error('Connection refused')
    return [folder]
  }, onResult: row => published.push(row.friendId) })
  assert.deepEqual(new Set(published), new Set(['linux', 'mac']))
  assert.equal(result[0].error, 'Linux Box: Connection refused')
  assert.deepEqual(result[0].locations, [folder])
  assert.equal(result[0].status, 'error')
  assert.equal(result[1].error, null)
  assert.equal(result[1].status, 'ready')
})
test('successful reload replaces revoked locations and clears a previous failure', async () => {
  const result = await loadLocations({ friends: [friends[0]], online: () => true, cached: { linux: [folder] }, list: async () => [] })
  assert.deepEqual(result[0].locations, [])
  assert.equal(result[0].error, null)
})
test('probe errors are isolated and empty friend lists complete', async () => {
  assert.deepEqual(await loadLocations({ friends: [], online: () => false, list: async () => [] }), [])
  const result = await loadLocations({ friends: [friends[0]], online: () => false, probe: async () => { throw new Error('timeout') }, list: async () => { throw new Error('must not list') } })
  assert.equal(result[0].error, 'Linux Box: timeout')
  assert.equal(result[0].status, 'error')
})
test('native rows: pending, checking, no address, offline keeps last known list, fresh list counts as online', async () => {
  const now = 1_000_000
  const rows = (results: Record<string, any>, checking: string[] = [], presence = () => false) =>
    nativeLocationRows(friends, { presence, results, shared: { linux: [folder] }, checking: new Set(checking), now })
  const first = rows({}, ['linux', 'legacy'])
  assert.deepEqual(first.map(r => [r.status, r.checking]), [['pending', true], ['pending', false], ['unavailable', false]])
  assert.match(first[2].error!, /hasn’t connected/)
  const listed = rows({ linux: { status: 'ready', error: null, at: now - 1000 }, mac: { status: 'offline', error: 'Mac is offline', at: now } })
  assert.equal(listed[0].online, true, 'a list that answered a second ago is contact')
  assert.deepEqual(listed[0].locations, [folder])
  assert.equal(listed[1].status, 'offline')
  assert.equal(listed[1].error, null, 'offline is a state, not an error message')
  const stale = rows({ linux: { status: 'ready', error: null, at: now - 600_000 } })
  assert.equal(stale[0].online, false)
  const failed = rows({ mac: { status: 'error', error: 'This device didn’t answer.', at: now } }, [], () => true)
  assert.equal(failed[1].error, 'This device didn’t answer.')
  assert.equal(failed[1].online, true)
})
