import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import type { TransferUpdate } from '../src/lib/api.ts'
import { outgoingForLocation } from '../src/lib/locationUploads.ts'

const t = (id: string, patch: Partial<TransferUpdate> = {}) =>
  ({ id, direction: 'send', state: 'transferring', fileNames: ['a'], bytesDone: 1, bytesTotal: 2, ...patch }) as TransferUpdate

test('outgoing Location uploads are matched by friend + location and stay while paused (#30)', () => {
  const known = {
    a: { friendId: 'f1', locationId: 'nas', relPath: 'Photos' },
    b: { friendId: 'f1', locationId: 'other', relPath: '' },
    c: { friendId: 'f2', locationId: 'nas', relPath: '' },
    d: { friendId: 'f1', locationId: 'nas', relPath: '' },
    e: { friendId: 'f1', locationId: 'nas', relPath: '' },
  }
  const transfers = { a: t('a'), b: t('b'), c: t('c'), d: t('d', { state: 'paused' }), e: t('e', { state: 'completed' }) }
  const got = outgoingForLocation(transfers, 'f1', 'nas', known)
  assert.deepEqual(got.map((u) => u.id), ['a', 'd'])
  assert.equal(got[0].relPath, 'Photos')
  // A receive with the same id never counts, nor does an id with no card.
  assert.deepEqual(outgoingForLocation({ a: t('a', { direction: 'receive' }) }, 'f1', 'nas', known), [])
})
