import { strict as assert } from 'node:assert'
import { readFileSync } from 'node:fs'
import { test } from 'node:test'
import ts from 'typescript'

// Node cannot strip .tsx directly. This module is deliberately pure: no JSX,
// React, store or Tauri imports; compile the real helper with the installed TS.
const source = readFileSync(new URL('../src/mobile/screens/HistorySections.tsx', import.meta.url), 'utf8')
const { outputText } = ts.transpileModule(source, { compilerOptions: { target: ts.ScriptTarget.ES2023, module: ts.ModuleKind.ESNext } })
const { groupHistoryDays } = await import(`data:text/javascript;base64,${Buffer.from(outputText).toString('base64')}`)
const at = (year: number, month: number, day: number, hour = 12) => new Date(year, month - 1, day, hour).getTime()

test('recents group by each calendar day, newest first, without mutating history', () => {
  const entries = [
    { id: 'older', timestampMs: at(2026, 9, 15) },
    { id: 'yesterday', timestampMs: at(2026, 9, 20) },
    { id: 'morning', timestampMs: at(2026, 9, 21, 9) },
    { id: 'afternoon', timestampMs: at(2026, 9, 21, 15) },
    { id: 'another-day', timestampMs: at(2026, 9, 14) },
  ]
  const before = structuredClone(entries)
  const groups = groupHistoryDays(entries, new Date(2026, 8, 21, 18))
  assert.deepEqual(groups.map(g => g.label), ['TODAY', 'YESTERDAY', 'TUESDAY, SEPTEMBER 15', 'MONDAY, SEPTEMBER 14'])
  assert.deepEqual(groups[0].entries.map(e => e.id), ['afternoon', 'morning'])
  assert.deepEqual(entries, before)
})

test('midnight, month and year boundaries use local dates', () => {
  const groups = groupHistoryDays([
    { timestampMs: at(2026, 1, 1, 0) },
    { timestampMs: at(2025, 12, 31, 23) },
    { timestampMs: at(2025, 12, 30) },
  ], new Date(2026, 0, 1, 1))
  assert.deepEqual(groups.map(g => g.label), ['TODAY', 'YESTERDAY', 'TUESDAY, DECEMBER 30, 2025'])
  assert.deepEqual(groupHistoryDays([], new Date()), [])
})

test('yesterday follows the calendar across daylight-saving transitions', () => {
  const previousTZ = process.env.TZ
  try {
    process.env.TZ = 'America/New_York'
    for (const [month, day] of [[3, 9], [11, 2]]) {
      const now = new Date(2026, month - 1, day, 12)
      const yesterday = new Date(2026, month - 1, day - 1, 0, 30)
      const twoDaysAgo = new Date(2026, month - 1, day - 2, 23, 30)
      const groups = groupHistoryDays([{ timestampMs: twoDaysAgo.getTime() }, { timestampMs: yesterday.getTime() }], now)
      assert.equal(groups[0].label, 'YESTERDAY')
      assert.notEqual(groups[1].label, 'YESTERDAY')
    }
  } finally { if (previousTZ === undefined) delete process.env.TZ; else process.env.TZ = previousTZ }
})
