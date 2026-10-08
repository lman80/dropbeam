import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { formatBytes, formatBytesLive, formatSpeed } from '../src/lib/format.ts'

test('above 1 GB the counter keeps two decimals so it visibly moves', () => {
  // GitHub #25: 5.1 GB of 56.6 GB looked frozen for minutes at a time.
  assert.equal(formatBytesLive(5.114e9), '5.11 GB')
  assert.equal(formatBytesLive(5.117e9), '5.12 GB')
  assert.equal(formatBytesLive(56.653e9), '56.65 GB')
  assert.equal(formatBytesLive(2.5e12), '2.50 TB')
  // A static size reads like Finder: no trailing zeros.
  assert.equal(formatBytes(2.5e12), '2.5 TB')
  assert.notEqual(formatBytesLive(5.114e9), formatBytesLive(5.124e9))
})

test('static sizes read like Finder (no trailing zeros); live ones keep fixed decimals', () => {
  assert.equal(formatBytes(0), 'Zero bytes')
  assert.equal(formatBytes(-1), 'Zero bytes')
  assert.equal(formatBytes(999), '999 bytes')
  assert.equal(formatBytes(1500), '1.5 KB')
  assert.equal(formatBytes(640e3), '640 KB')
  assert.equal(formatBytes(2e6), '2 MB')
  assert.equal(formatBytes(14.2e9), '14.2 GB')
  assert.equal(formatBytes(1.25e6), '1.3 MB')
  assert.equal(formatBytes(999e6), '999 MB')
  assert.equal(formatBytes(999_999), '1 MB')
  assert.equal(formatBytesLive(1.25e6), '1.3 MB')
  assert.equal(formatBytesLive(2e6), '2.0 MB')
  // An explicit request still wins.
  assert.equal(formatBytes(5.114e9, 1), '5.1 GB')
})

test('speeds stay at one decimal in both units', () => {
  assert.equal(formatSpeed(12.34e6, false), '12.3 MB/s')
  assert.equal(formatSpeed(1.234e9, false), '1.2 GB/s')
  assert.equal(formatSpeed(0, false), '—')
  assert.equal(formatSpeed(12.5e6, true), '100 Mbps')
})

test('pathKind: a stale "connecting" snapshot is ignored while bytes are moving (#30)', async () => {
  const { pathKind } = await import('../src/lib/humanize.ts')
  const connecting = { path: 'connecting', rttMs: null, upgrading: false, relay: null } as never
  assert.equal(pathKind(connecting, null), 'connecting')
  assert.equal(pathKind(connecting, 'local', true), 'local')
  assert.equal(pathKind(connecting, 'internet', true), 'relay')
  assert.equal(pathKind(connecting, null, true), null)
})
