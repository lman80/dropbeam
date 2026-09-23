import test from 'node:test'
import assert from 'node:assert/strict'
import { nativeFolders, folderLinks, folderStatusLine } from '../src/lib/nativeFolders.ts'
import type { FolderStatus, Pair } from '../src/lib/api.ts'

const pair = (p: Partial<Pair>): Pair => ({ id: 'p', role: 'b', peerName: 'Mong', secret: 's', folder: '/Documents/Imported Folders/u/Vacation', twoWay: true, mirror: true, autoDelete: false, deleteMode: 'trash', createdAt: 1, endpointId: null, groupId: null, ...p })
const status = (s: Partial<FolderStatus>): FolderStatus => ({ pairId: 'p', state: 'idle', queued: 0, sendingFile: null, percent: 0, bytesDone: 0, bytesTotal: 0, speedBps: 0, etaSeconds: null, detail: null, peerOnline: true, peerName: 'Mong', locality: 'direct', ...s })

test('group links collapse into one folder with every member; 1:1 folders stay separate', () => {
  const pairs = [
    pair({ id: 'a1', groupId: 'g', peerName: 'Mong', endpointId: 'e-mong', ownerEid: 'me' }),
    pair({ id: 'a2', groupId: 'g', peerName: '', role: 'a', ownerEid: 'me' }),
    pair({ id: 'solo', folder: 'C:\\Users\\x\\Work\\', mirror: false, twoWay: false, role: 'a' }),
  ]
  const out = nativeFolders(pairs, {}, {}, {}, 'me', [{ id: 'f-mong', endpointId: 'e-mong' }])
  assert.equal(out.length, 2)
  const g = out[0]
  assert.equal(g.id, 'g'); assert.equal(g.name, 'Vacation'); assert.equal(g.iAmOwner, true)
  assert.deepEqual(g.members.map(m => [m.pairId, m.name, m.pending, m.canSetRole, m.friendId]), [['a1', 'Mong', false, true, 'f-mong'], ['a2', 'Waiting to join…', true, false, null]])
  assert.equal(g.pendingInvite, 'a2')
  assert.equal(out[1].name, 'Work'); assert.equal(out[1].mode, 'sendOnly'); assert.equal(out[1].modeLabel, 'View only (they receive)')
})

test('roles are only settable by the recorded owner', () => {
  const [f] = nativeFolders([pair({ id: 'x', ownerEid: 'someone-else' })], {}, {}, {}, 'me')
  assert.equal(f.iAmOwner, false)
  assert.equal(f.members[0].canSetRole, false)
  const [legacy] = nativeFolders([pair({ id: 'x', ownerEid: null })], {}, {}, {}, null)
  assert.equal(legacy.iAmOwner, false)
})

test('the busiest member link drives the folder status and progress', () => {
  const pairs = [pair({ id: 'a', groupId: 'g' }), pair({ id: 'b', groupId: 'g', peerName: 'Sam' })]
  const [f] = nativeFolders(pairs, { a: status({ pairId: 'a' }), b: status({ pairId: 'b', state: 'sending', sendingFile: 'IMG_1.heic', percent: 140, bytesDone: 5, bytesTotal: 10, etaSeconds: Infinity, queuedFiles: ['IMG_2.heic'] }) })
  assert.equal(f.state, 'sending'); assert.equal(f.tone, 'busy'); assert.equal(f.label, 'Sending IMG_1.heic')
  assert.equal(f.percent, 100); assert.equal(f.etaSeconds, null); assert.deepEqual(f.queuedFiles, ['IMG_2.heic'])
  assert.equal(f.locality, 'direct'); assert.equal(f.inSync, false); assert.equal(f.summary, null)
})

test('status wording matches desktop: paused, waiting for invite, offline, in sync', () => {
  assert.equal(folderStatusLine(pair({}), status({ paused: true })).label, 'Sync paused — Resume to merge changes')
  assert.equal(folderStatusLine(pair({ role: 'a', peerName: '' }), status({ peerOnline: false })).label, 'Waiting for someone to accept the invite')
  assert.equal(folderStatusLine(pair({}), status({ peerOnline: false })).tone, 'offline')
  assert.equal(folderStatusLine(pair({}), status({ state: 'waiting', detail: null, queued: 2 })).label, 'Waiting for Mong · 2 queued')
  const [f] = nativeFolders([pair({ id: 'p' })], { p: status({ peerFiles: 12 }) }, { p: { pairId: 'p', direction: 'receive', files: 3, bytes: 9, durationMs: 1000, avgBps: 9 } }, { p: 1700 })
  assert.equal(f.inSync, true); assert.equal(f.peerFiles, 12); assert.equal(f.lastSyncedMs, 1700)
  assert.deepEqual(f.summary, { direction: 'receive', files: 3, bytes: 9, durationMs: 1000, avgBps: 9 })
})

test('a paused member pauses the whole folder; leaving targets every link and rejects stale ids', () => {
  const pairs = [pair({ id: 'a', groupId: 'g' }), pair({ id: 'b', groupId: 'g' }), pair({ id: 'c' })]
  assert.equal(nativeFolders(pairs, { b: status({ pairId: 'b', paused: true }) })[0].paused, true)
  assert.deepEqual(folderLinks(pairs, 'g').map(p => p.id), ['a', 'b'])
  assert.deepEqual(folderLinks(pairs, 'c').map(p => p.id), ['c'])
  assert.throws(() => folderLinks(pairs, 'gone'))
})
