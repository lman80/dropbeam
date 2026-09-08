import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import type { ChatMessage, TransferUpdate } from '../src/lib/api.ts'
import { normalizeChatMessage, normalizeTransfer } from '../src/lib/normalize.ts'
import { formatBytes, formatBytesLive, formatEta, formatRelativeTime, formatSpeed } from '../src/lib/format.ts'
import { LandedEta } from '../src/lib/eta.ts'

test('old chat events and stored messages remain safe for text, file and search rendering', () => {
  for (const kind of ['text', 'file'] as const) {
    const m = normalizeChatMessage({ id: 'old', peerId: 'friend', kind } as ChatMessage)
    assert.equal(m.text, '')
    assert.deepEqual(m.files, [])
    assert.deepEqual(m.reactions, [])
    assert.equal([...m.text.matchAll(/https?:\/\/\S+/g)].length, 0)
    assert.equal(m.text.toLowerCase().includes('needle') || m.files.some((f) => f.includes('needle')), false)
    assert.equal(m.seq, 0)
    assert.equal(m.ts, 0)
  }
})

test('zero totals, missing speed and landed zero have finite display data and no fabricated ETA', () => {
  const t = normalizeTransfer({ id: 'incoming', state: 'transferring', bytesTotal: 0, bytesDone: 0 } as TransferUpdate)
  assert.deepEqual(t.fileNames, [])
  assert.equal(t.percent, 0)
  assert.equal(formatSpeed(t.speedBps), '—')
  assert.equal(formatEta(t.etaSeconds), '—')
  const eta = new LandedEta()
  for (const n of [0, NaN, Infinity, undefined]) {
    assert.equal(eta.update(1000, n as number, 100), null)
  }
})

test('invalid counters never leak NaN/Infinity into labels or animation widths', () => {
  for (const n of [NaN, Infinity, -Infinity, undefined]) {
    const t = normalizeTransfer({ bytesDone: n, bytesTotal: n, speedBps: n, percent: n } as TransferUpdate)
    assert.equal(t.percent, 0)
    assert.equal(formatBytes(t.bytesTotal), '0 B')
    assert.equal(formatBytesLive(n as number), '0 B')
    assert.equal(formatSpeed(n as number), '—')
    assert.equal(formatEta(n), '—')
    assert.equal(formatRelativeTime(n as number), '—')
  }
  assert.equal(formatBytes(0.5), '1 B')
  assert.equal(formatBytes(1500), '1.5 kB')
  assert.equal(normalizeTransfer({ bytesDone: 50, bytesTotal: 100 } as TransferUpdate).percent, 50)
  assert.equal(normalizeTransfer({ percent: 150 } as TransferUpdate).percent, 100)
})

test('sparse transfer events retain known names', () => {
  const prev = normalizeTransfer({ id: 'send', fileNames: ['report.txt'] } as TransferUpdate)
  const failed = normalizeTransfer({ id: 'send', state: 'failed' } as TransferUpdate, prev)
  assert.deepEqual(failed.fileNames, ['report.txt'])
})
