import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import type { SyncedFolder, SyncedFolderStatus } from '../src/lib/api.ts'
import { destinationLabel, folderName, statusPill, summarySentence } from '../src/lib/syncedFolders.ts'

const folder = (over: Partial<SyncedFolder> = {}): SyncedFolder => ({
  id: 'sf1',
  friendId: 'f1',
  locationId: 'loc1',
  relPath: 'Travel',
  localPath: '/Users/you/Pictures/Travel',
  enabled: true,
  deleteRemote: false,
  createdAt: 1,
  lastCheckAt: 2,
  lastResult: null,
  ...over,
})
const status = (over: Partial<SyncedFolderStatus> = {}): SyncedFolderStatus => ({
  id: 'sf1', state: 'idle', pendingFiles: 0, lastCheckAt: 2, message: '', transferId: null, ...over,
})

test('a paused folder says so no matter what the engine last reported', () => {
  assert.deepEqual(statusPill(folder({ enabled: false }), status({ state: 'uploading', pendingFiles: 9 })),
    { tone: 'off', label: 'Paused' })
})

test('an upload counts the files still to move, in plain words', () => {
  assert.deepEqual(statusPill(folder(), status({ state: 'uploading', pendingFiles: 12 })),
    { tone: 'busy', label: 'Copying 12 files' })
  assert.deepEqual(statusPill(folder(), status({ state: 'uploading', pendingFiles: 1 })),
    { tone: 'busy', label: 'Copying 1 file' })
  // Nothing counted yet — never "Copying 0 files".
  assert.deepEqual(statusPill(folder(), status({ state: 'uploading', pendingFiles: 0 })),
    { tone: 'busy', label: 'Copying' })
})

test('an unreachable host reads as a wait, not an error', () => {
  const pill = statusPill(folder(), status({ state: 'waiting', message: 'Waiting for Linux Box' }))
  assert.equal(pill.tone, 'wait')
  assert.equal(pill.label, 'Waiting for Linux Box')
})

test('a healthy folder and a broken one are told apart', () => {
  assert.deepEqual(statusPill(folder(), status({ state: 'idle', message: 'Up to date' })),
    { tone: 'ok', label: 'Up to date' })
  assert.deepEqual(statusPill(folder(), status({ state: 'idle', message: '' })),
    { tone: 'ok', label: 'Up to date' })
  assert.deepEqual(statusPill(folder(), status({ state: 'error', message: 'Ashton isn’t sharing that folder any more' })),
    { tone: 'bad', label: 'Ashton isn’t sharing that folder any more' })
  assert.equal(statusPill(folder(), status({ state: 'scanning' })).label, 'Checking for new files')
})

test('before the engine reports, the card falls back to what was written down', () => {
  assert.deepEqual(statusPill(folder({ lastCheckAt: 0 }), undefined), { tone: 'wait', label: 'Getting ready…' })
  assert.deepEqual(statusPill(folder(), undefined), { tone: 'wait', label: 'Up to date' })
  assert.deepEqual(statusPill(folder({ lastResult: { ok: false, message: 'Can’t find that folder' } }), undefined),
    { tone: 'bad', label: 'Can’t find that folder' })
})

test('the destination reads as "location › subfolder", or just the location', () => {
  assert.equal(destinationLabel('Buddy NAS', 'Travel'), 'Buddy NAS › Travel')
  assert.equal(destinationLabel('Buddy NAS', '/Photos/Travel/'), 'Buddy NAS › Photos/Travel')
  assert.equal(destinationLabel('Buddy NAS', ''), 'Buddy NAS')
})

test('the folder name survives trailing slashes and Windows paths', () => {
  assert.equal(folderName('/Users/you/Pictures/Travel'), 'Travel')
  assert.equal(folderName('/Users/you/Pictures/Travel/'), 'Travel')
  assert.equal(folderName('C:\\Users\\you\\Travel'), 'Travel')
})

test('the confirm sentence says exactly what happens to deletes', () => {
  const keep = summarySentence('/Users/you/Pictures/Travel', 'Buddy NAS', 'Travel', false)
  assert.match(keep, /Everything you put in “Travel” on this device will be copied to Buddy NAS › Travel\./)
  assert.match(keep, /Files you delete here stay on Buddy NAS\./)
  const mirror = summarySentence('/Users/you/Pictures/Travel', 'Buddy NAS', '', true)
  assert.match(mirror, /copied to Buddy NAS\./)
  assert.match(mirror, /moved to Buddy NAS's trash too\./)
})
