import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { formatBytes, formatBytesLive, formatSpeed } from '../src/lib/format.ts'

test('above 1 GB the counter keeps two decimals so it visibly moves', () => {
  // GitHub #25: 5.1 GB of 56.6 GB looked frozen for minutes at a time.
  assert.equal(formatBytesLive(5.114e9), '5.11 GB')
  assert.equal(formatBytesLive(5.117e9), '5.12 GB')
  assert.equal(formatBytesLive(56.653e9), '56.65 GB')
  assert.equal(formatBytes(2.5e12), '2.50 TB')
  assert.notEqual(formatBytesLive(5.114e9), formatBytesLive(5.124e9))
})

test('below 1 GB keeps the old formatting', () => {
  assert.equal(formatBytes(0), '0 B')
  assert.equal(formatBytes(-1), '0 B')
  assert.equal(formatBytes(999), '999 B')
  assert.equal(formatBytes(1500), '1.5 kB')
  assert.equal(formatBytes(1.25e6), '1.3 MB')
  assert.equal(formatBytes(999e6), '999.0 MB')
  assert.equal(formatBytesLive(1.25e6), '1.3 MB')
  // An explicit request still wins.
  assert.equal(formatBytes(5.114e9, 1), '5.1 GB')
})

test('speeds stay at one decimal in both units', () => {
  assert.equal(formatSpeed(12.34e6, false), '12.3 MB/s')
  assert.equal(formatSpeed(1.234e9, false), '1.2 GB/s')
  assert.equal(formatSpeed(0, false), '—')
  assert.equal(formatSpeed(12.5e6, true), '100 Mbps')
})
