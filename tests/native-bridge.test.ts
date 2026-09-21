import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { changedSnapshots, dispatchNativeCall } from '../src/lib/nativeBridgeProtocol.ts'

test('native calls preserve correlation and JSON arguments including quotes and paths', async () => {
  const args = { name: '"; window.alert(1); //', paths: ['/tmp/a\nb.txt'] }
  assert.deepEqual(await dispatchNativeCall({ echo: a => a }, 12, 'echo', args), { id: 12, ok: true, value: args })
})
test('void results become JSON null and action errors become failed replies', async () => {
  assert.deepEqual(await dispatchNativeCall({ done: async () => {} }, 1, 'done', {}), { id: 1, ok: true, value: null })
  assert.deepEqual(await dispatchNativeCall({ fail: () => { throw new Error('Offline') } }, 2, 'fail', {}), { id: 2, ok: false, value: 'Offline' })
})
test('unknown and prototype handler names are rejected without calling them', async () => {
  for (const name of ['missing', 'constructor', 'toString', '__proto__']) {
    assert.equal((await dispatchNativeCall({}, 1, name, {})).ok, false)
  }
})
test('snapshots publish all keys initially and only changed JSON thereafter', () => {
  const previous = new Map<string, string>()
  const initial = { friends: [], transfers: [], settings: null, chatOverview: [], presence: {}, myDevice: null }
  assert.equal(changedSnapshots(previous, initial).length, 6)
  assert.deepEqual(changedSnapshots(previous, { ...initial }), [])
  assert.deepEqual(changedSnapshots(previous, { ...initial, presence: { alice: true } }), [{ key: 'presence', value: { alice: true } }])
  assert.deepEqual(changedSnapshots(previous, { ...initial }), [{ key: 'presence', value: {} }])
})
test('concurrent native calls keep their original IDs when replies finish out of order', async () => {
  let finish: (value: string) => void = () => {}
  const slow = dispatchNativeCall({ slow: () => new Promise<string>(resolve => { finish = resolve }) }, 7, 'slow', {})
  assert.deepEqual(await dispatchNativeCall({ fast: () => 'second' }, 8, 'fast', {}), { id: 8, ok: true, value: 'second' })
  finish('first')
  assert.deepEqual(await slow, { id: 7, ok: true, value: 'first' })
})
