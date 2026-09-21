import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { loadLocations } from '../src/lib/locationsLoad.ts'

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
    { friendId: 'linux', friendName: 'Linux Box', online: true, locations: [folder], error: null },
    { friendId: 'mac', friendName: 'Mac', online: false, locations: [], error: 'Mac is offline' },
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
  assert.equal(result[1].error, null)
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
})
