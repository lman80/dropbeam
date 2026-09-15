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
