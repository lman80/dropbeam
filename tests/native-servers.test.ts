import test from 'node:test'
import assert from 'node:assert/strict'
import { nativePendingFiles, nativeServerList, serverPrefsArgs } from '../src/lib/nativeServers.ts'

test('server list drops malformed/duplicate rows and never holds on an unused server', () => {
  const list = nativeServerList([
    { eid: 'a', name: ' Linux Box ', useIt: true, holdForMe: true, offer: 'new', learnedMs: 5 },
    { eid: 'a', name: 'dup' },
    { eid: 'b', name: '', useIt: false, holdForMe: true, offer: 'weird', revoked: 'yes' },
    { name: 'no id' }, null, 'x',
  ])
  assert.equal(list.length, 2)
  assert.deepEqual([list[0].name, list[0].holdForMe, list[0].offer], ['Linux Box', true, 'new'])
  assert.deepEqual([list[1].name, list[1].holdForMe, list[1].offer, list[1].revoked], ['Transfer Server', false, '', false])
  assert.deepEqual(nativeServerList({}), [])
})

test('pending files need a link id and keep only string names', () => {
  const list = nativePendingFiles([{ linkId: 'receive:x', serverName: '', names: ['a.zip', 3], bytes: 'big' }, { names: ['b'] }])
  assert.equal(list.length, 1)
  assert.deepEqual([list[0].serverName, list[0].names, list[0].bytes], ['the Transfer Server', ['a.zip'], 0])
})

test('server prefs validate types, mark the offer seen, and switch holding off with use', () => {
  assert.deepEqual(serverPrefsArgs({ eid: 'a', useIt: false }), { eid: 'a', prefs: { useIt: false, holdForMe: false, offer: 'seen' } })
  assert.deepEqual(serverPrefsArgs({ eid: 'a', offer: 'dismissed' }), { eid: 'a', prefs: { offer: 'dismissed' } })
  assert.deepEqual(serverPrefsArgs({ eid: 'a', useIt: true, holdForMe: true, offer: 'seen' }).prefs, { useIt: true, holdForMe: true, offer: 'seen' })
  assert.throws(() => serverPrefsArgs({ eid: '', useIt: true }))
  assert.throws(() => serverPrefsArgs({ eid: 'a', useIt: 'yes' }))
  assert.throws(() => serverPrefsArgs({ eid: 'a', offer: 'new' }))
  assert.throws(() => serverPrefsArgs({ eid: 'a' }))
})
