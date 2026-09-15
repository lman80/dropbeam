import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import type { TransferUpdate } from '../src/lib/api.ts'
import { normalizeTransfer } from '../src/lib/normalize.ts'

test('Location skipped count survives sparse progress and completion updates', () => {
  const initial = normalizeTransfer({ id: 'nas', locationSkipped: 12 } as TransferUpdate)
  const progress = normalizeTransfer({ id: 'nas', state: 'transferring', locationSkipped: null } as TransferUpdate, initial)
  const complete = normalizeTransfer({ id: 'nas', state: 'completed' } as TransferUpdate, progress)
  assert.equal(complete.locationSkipped, 12)
  assert.equal(normalizeTransfer({ id: 'nas', locationSkipped: 0 } as TransferUpdate, complete).locationSkipped, 0)
  assert.equal(normalizeTransfer({ id: 'old' } as TransferUpdate).locationSkipped, undefined)
})

test('A paused update keeps the progress and recipient the card needs to resume', () => {
  const sending = normalizeTransfer({
    id: 'send-1', direction: 'send', state: 'transferring', friendName: 'Mong',
    fileNames: ['clip.mov'], fileCount: 1, bytesDone: 400, bytesTotal: 1000, percent: 40,
  } as TransferUpdate)

  // The engine's Paused snapshot carries its own counts.
  const paused = normalizeTransfer({
    id: 'send-1', direction: 'send', state: 'paused',
    bytesDone: 400, bytesTotal: 1000, detail: 'Paused — resume any time',
  } as TransferUpdate, sending)
  assert.equal(paused.bytesDone, 400)
  assert.equal(paused.bytesTotal, 1000)
  assert.equal(paused.percent, 40)
  // Retry data: who it was going to and what it was, so Resume can replay it.
  assert.equal(paused.friendName, 'Mong')
  assert.deepEqual(paused.fileNames, ['clip.mov'])
  assert.equal(paused.fileCount, 1)

  // A bare Paused snapshot (staged send / older payload) inherits the card's own.
  const bare = normalizeTransfer({ id: 'send-1', direction: 'send', state: 'paused' } as TransferUpdate, sending)
  assert.equal(bare.bytesDone, 400)
  assert.equal(bare.bytesTotal, 1000)
  assert.equal(bare.percent, 40)
  assert.equal(bare.friendName, 'Mong')

  // Nothing is inherited for any OTHER state — a fresh transfer starts at zero.
  const canceled = normalizeTransfer({ id: 'send-1', direction: 'send', state: 'canceled' } as TransferUpdate, sending)
  assert.equal(canceled.bytesDone, 0)
  assert.equal(canceled.bytesTotal, 0)
  assert.equal(canceled.friendName, null)
})

test('Location conflict count survives sparse progress and completion updates', () => {
  const initial = normalizeTransfer({ id: 'nas', locationConflicts: 3 } as TransferUpdate)
  const progress = normalizeTransfer({ id: 'nas', state: 'transferring', locationConflicts: null } as TransferUpdate, initial)
  const complete = normalizeTransfer({ id: 'nas', state: 'completed' } as TransferUpdate, progress)
  assert.equal(complete.locationConflicts, 3)
  // A later push can raise the count; an explicit 0 still clears it.
  assert.equal(normalizeTransfer({ id: 'nas', locationConflicts: 5 } as TransferUpdate, complete).locationConflicts, 5)
  assert.equal(normalizeTransfer({ id: 'nas', locationConflicts: 0 } as TransferUpdate, complete).locationConflicts, 0)
  // An older host never sends the field: it must stay undefined, not 0.
  assert.equal(normalizeTransfer({ id: 'old' } as TransferUpdate).locationConflicts, undefined)
  // Skipped and conflicts are independent counters.
  const both = normalizeTransfer({ id: 'nas', locationSkipped: 2 } as TransferUpdate, complete)
  assert.equal(both.locationSkipped, 2)
  assert.equal(both.locationConflicts, 3)
})
