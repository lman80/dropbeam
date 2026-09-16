import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import type { TransferUpdate } from '../src/lib/api.ts'
import { incomingByLocation, trackLocationTransfers } from '../src/lib/hostedLocations.ts'

const xfer = (u: Partial<TransferUpdate> & { id: string }): TransferUpdate => ({
  id: u.id,
  direction: 'receive',
  state: 'transferring',
  code: null,
  fileNames: [],
  fileCount: 0,
  percent: 0,
  bytesDone: 0,
  bytesTotal: 0,
  speedBps: 0,
  etaSeconds: null,
  locality: 'unknown',
  peer: null,
  error: null,
  outDir: null,
  friendName: null,
  ...u,
} as TransferUpdate)

test('a location id seen once on a receive is remembered for the rest of that transfer', () => {
  const first = trackLocationTransfers({}, { a: xfer({ id: 'a', locationId: 'nas' }) })
  assert.deepEqual(first, { a: 'nas' })
  // Later progress ticks drop the field; the mapping must survive them.
  const later = trackLocationTransfers(first, { a: xfer({ id: 'a', bytesDone: 500 }) })
  assert.deepEqual(later, { a: 'nas' })
  assert.equal(later, first, 'an unchanged map is returned by identity so React can skip the render')
})

test('transfers that leave the list are forgotten, and unrelated ones are never tracked', () => {
  const known = trackLocationTransfers({}, { a: xfer({ id: 'a', locationId: 'nas' }) })
  assert.deepEqual(trackLocationTransfers(known, {}), {})
  assert.deepEqual(trackLocationTransfers({}, { b: xfer({ id: 'b' }) }), {}, 'a plain friend receive has no location')
  const two = trackLocationTransfers(known, {
    a: xfer({ id: 'a' }),
    c: xfer({ id: 'c', locationId: 'photos' }),
  })
  assert.deepEqual(two, { a: 'nas', c: 'photos' })
})

test('live inbound uploads are grouped per hosted folder and ignore sends and finished pushes', () => {
  const known = { a: 'nas', b: 'nas', c: 'nas', d: 'photos', e: 'nas' }
  const rows = incomingByLocation({
    a: xfer({ id: 'a', friendName: 'Sam', fileCount: 3, bytesTotal: 1_000_000 }),
    b: xfer({ id: 'b', friendName: 'Sam', fileCount: 1, bytesTotal: 500_000 }),
    c: xfer({ id: 'c', friendName: 'Mong', fileCount: 2, bytesTotal: 2_000_000 }),
    // A completed push and an outbound send must not show as "receiving".
    d: xfer({ id: 'd', friendName: 'Sam', state: 'completed', fileCount: 9, bytesTotal: 9 }),
    e: xfer({ id: 'e', friendName: 'Sam', direction: 'send', fileCount: 9, bytesTotal: 9 }),
    // Untracked (no location) — belongs to the ordinary Downloads receive list.
    f: xfer({ id: 'f', friendName: 'Sam', fileCount: 4, bytesTotal: 4 }),
  }, known)
  assert.deepEqual(Object.keys(rows), ['nas'])
  assert.deepEqual(rows.nas, { friends: ['Sam', 'Mong'], files: 6, bytes: 3_500_000 })
})

test('a nameless sender still reads as a friend, and fileNames stand in for a missing count', () => {
  const rows = incomingByLocation(
    { a: xfer({ id: 'a', fileNames: ['one.txt', 'two.txt'] }) },
    { a: 'nas' },
  )
  assert.deepEqual(rows.nas, { friends: ['a friend'], files: 2, bytes: 0 })
})
