import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { integrityRows, mergeIntegrity } from '../src/lib/integrity.ts'
import type { FileIntegrity } from '../src/lib/api.ts'
const row = (index: number, size: number): FileIntegrity => ({ index, name: 'same.bin', size,
  algorithm: 'SHA-256', digest: 'a'.repeat(64), peerDigest: 'a'.repeat(64), verified: true, acknowledged: true })
test('duplicate names in successive splits stay verified through a retry and reload', () => {
  const merged = mergeIntegrity(mergeIntegrity([], [row(0, 1)]), [row(1, 2)])
  const retried = mergeIntegrity(merged, [row(0, 1)])
  const restored = integrityRows(JSON.parse(JSON.stringify(retried)))
  assert.equal(restored.length, 2)
  assert.equal(restored.reduce((n, r) => n + r.size, 0), 3)
  assert.ok(restored.every(r => r.verified && r.acknowledged))
})
test('a matched checksum without receipt acknowledgement remains unconfirmed', () => {
  const rows = integrityRows([{ ...row(0, 1), acknowledged: false }])
  assert.equal(rows.length, 1)
  assert.equal(rows[0].acknowledged, false)
  assert.equal(integrityRows([{ ...row(0, 1), index: -1 }]).length, 0)
})
