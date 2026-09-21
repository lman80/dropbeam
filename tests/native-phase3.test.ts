import test from 'node:test'
import assert from 'node:assert/strict'
import { locationChild, nativeBrowserPage, nativeHistoryPaths, requireLocationRight } from '../src/lib/nativePhase3.ts'
import { changedSnapshots } from '../src/lib/nativeBridgeProtocol.ts'

test('native browser advances with the next opaque cursor, never repeats the current page', () => {
  const first = nativeBrowserPage({ entries: [{ name: 'one', isDir: false, size: 4, modified: 10 }], cursor: 'current', nextCursor: 'next', hasMore: true, total: 500 })
  assert.equal(first.cursor, 'next')
  assert.equal(first.total, 500)
  assert.equal(nativeBrowserPage({ entries: [], cursor: 'last', hasMore: false }).cursor, null)
})
test('location actions reject removed locations and independent upload/manage grants', () => {
  const uploadOnly = { id: 'x', name: 'Inbox', rights: { upload: true, manage: false } }
  assert.doesNotThrow(() => requireLocationRight(uploadOnly, 'read'))
  assert.doesNotThrow(() => requireLocationRight(uploadOnly, 'upload'))
  assert.throws(() => requireLocationRight(uploadOnly, 'manage'))
  assert.throws(() => requireLocationRight(undefined, 'read'))
  assert.throws(() => requireLocationRight({ ...uploadOnly, rights: { upload: false, manage: true } }, 'upload'))
})
test('browser child names preserve unicode but cannot escape a selected directory', () => {
  assert.equal(locationChild('photos/2026', '한강.jpg'), 'photos/2026/한강.jpg')
  for (const name of ['', ' ', '..', '.', '../private', 'dir/file', 'dir\\file', 'a\0b']) assert.throws(() => locationChild('photos', name))
})
test('history only exposes completed local paths and rejects traversal', () => {
  const entry = { id: 'x', direction: 'receive', fileNames: ['photo.jpg', '../secret', '/private', 'album/pic.jpg'], bytesTotal: 4, peer: null, locality: 'direct', code: null, state: 'completed', timestampMs: 1, error: null, outDir: '/downloads' } as const
  assert.deepEqual(nativeHistoryPaths({ ...entry, fileNames: [...entry.fileNames] }), ['/downloads/photo.jpg', '/downloads/album/pic.jpg'])
  assert.deepEqual(nativeHistoryPaths({ ...entry, fileNames: [...entry.fileNames], state: 'failed' }), [])
  assert.deepEqual(nativeHistoryPaths({ ...entry, fileNames: [...entry.fileNames], outDir: null }), [])
})
test('history and location snapshots deliver same-length edits and permission revocations', () => {
  const previous = new Map<string, string>()
  const snapshot = { history: [{ id: 'a', state: 'completed' }], locations: [{ id: 'l', rights: { manage: true } }] }
  assert.equal(changedSnapshots(previous, snapshot).length, 2)
  assert.equal(changedSnapshots(previous, snapshot).length, 0)
  snapshot.locations[0].rights.manage = false
  snapshot.history[0].state = 'failed'
  assert.deepEqual(changedSnapshots(previous, snapshot).map(s => s.key).sort(), ['history', 'locations'])
})
