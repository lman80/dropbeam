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
