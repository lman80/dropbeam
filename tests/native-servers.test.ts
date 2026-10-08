import test from 'node:test'
import assert from 'node:assert/strict'
import { nativePendingFiles, nativePushStatus, nativeServerList, serverPrefsArgs } from '../src/lib/nativeServers.ts'

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

test('push status coerces to calm defaults', () => {
  assert.deepEqual(nativePushStatus({ enabled: true, previews: false, servers: 2 }), { enabled: true, previews: false, servers: 2 })
  assert.deepEqual(nativePushStatus(null), { enabled: false, previews: true, servers: 0 })
  assert.deepEqual(nativePushStatus({ enabled: 'yes', previews: 0, servers: -3.5 }), { enabled: false, previews: true, servers: 0 })
})

test('servers shared by a friend (their own server) and the owner’s share question', () => {
  const list = nativeServerList([
    { eid: 'box', name: 'Linux Box', member: true, offer: 'new', via: ['ash-mac', 7], viaPeer: 'f1', viaName: ' Ashton ' },
    { eid: 'mine', name: 'Linux Box', owner: true, shareFriends: true, offer: 'share', access: 'all' },
    { eid: 'fake', shareFriends: true },
  ])
  assert.deepEqual([list[0].via, list[0].viaPeer, list[0].viaName, list[0].owner], [['ash-mac'], 'f1', 'Ashton', false])
  assert.deepEqual([list[1].owner, list[1].shareFriends, list[1].offer, list[1].access], [true, true, 'share', 'all'])
  assert.equal(list[2].shareFriends, false, 'only an owned server can be shared')
  assert.deepEqual(serverPrefsArgs({ eid: 'mine', shareFriends: true }).prefs, { shareFriends: true, offer: 'seen' })
  assert.throws(() => serverPrefsArgs({ eid: 'mine', offer: 'share' }))
})
