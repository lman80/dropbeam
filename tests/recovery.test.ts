import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { isRecoveryQr, listNames, makeQuiz, oldDeviceName, ordinal, restoreSummary, sheetLines, shouldRemind, splitWords } from '../src/lib/recovery.ts'

const WORDS = 'legal winner thank year wave sausage worth useful legal winner thank yellow'.split(' ')

function seeded(seed: number) {
  return () => { seed = (seed * 1103515245 + 12345) % 2 ** 31; return seed / 2 ** 31 }
}

test('ordinals read naturally', () => {
  assert.deepEqual([1, 2, 3, 4, 11, 12, 13, 21, 22, 23, 24].map(ordinal), ['1st', '2nd', '3rd', '4th', '11th', '12th', '13th', '21st', '22nd', '23rd', '24th'])
})

test('the quiz asks two different positions, each with the right word among four from the code', () => {
  for (let s = 1; s < 50; s++) {
    const quiz = makeQuiz(WORDS, seeded(s))
    assert.equal(quiz.length, 2)
    assert.notEqual(quiz[0].index, quiz[1].index)
    for (const q of quiz) {
      assert.ok(q.options.includes(WORDS[q.index]))
      assert.ok(q.options.length >= 3 && q.options.length <= 4)
      assert.equal(new Set(q.options).size, q.options.length, 'no repeated choices')
      assert.ok(q.options.every(o => WORDS.includes(o)))
    }
  }
  assert.deepEqual(makeQuiz(['a', 'b']), [])
})

test('pasted or scanned words are split however they were written', () => {
  assert.deepEqual(splitWords('1. Legal\n2. winner, 3) thank'), ['legal', 'winner', 'thank'])
  assert.deepEqual(splitWords('dropbeamrecover1:legal winner'), ['legal', 'winner'])
  assert.ok(isRecoveryQr(' DropBeamRecover1:abc') && !isRecoveryQr('dropbeam:abc'))
})

test('the reminder waits two weeks after "later" and stops once saved', () => {
  const now = 100 * 24 * 3600 * 1000
  assert.equal(shouldRemind(null, now), false)
  assert.equal(shouldRemind({ saved: true, hasAccount: true, laterAt: 0 }, now), false)
  assert.equal(shouldRemind({ saved: false, hasAccount: false, laterAt: 0 }, now), true)
  assert.equal(shouldRemind({ saved: false, hasAccount: true, laterAt: now - 3 * 24 * 3600 * 1000 }, now), false)
  assert.equal(shouldRemind({ saved: false, hasAccount: true, laterAt: now - 15 * 24 * 3600 * 1000 }, now), true)
})

test('old devices and the restore summary use plain words', () => {
  assert.equal(oldDeviceName({ model: 'iPhone 12', os: 'ios' }), 'Your iPhone 12')
  assert.equal(oldDeviceName({ os: 'macos' }), 'Your Mac')
  assert.equal(oldDeviceName({ os: 'ios', kind: 'tablet' }), 'Your iPad')
  assert.equal(oldDeviceName({}), 'Your computer')
  const base = { restoredAt: 1, oldDevices: [], folders: [] }
  assert.match(restoreSummary({ ...base, friendsSynced: 0, returned: [] }), /few hours/)
  assert.equal(restoreSummary({ ...base, friendsSynced: 1, returned: ['Fran'] }), '1 friend has found you again (Fran). Others will as their DropBeam opens.')
  assert.equal(listNames(['A', 'B']), 'A and B')
  assert.equal(listNames(['A', 'B', 'C']), 'A, B, and C')
  assert.equal(listNames(['A', 'B', 'C', 'D', 'E']), 'A, B, C and 2 more')
})

test('the printed sheet numbers words in two columns', () => {
  const lines = sheetLines(WORDS)
  assert.equal(lines.length, 6)
  assert.match(lines[0], /^ 1\. legal +7\. worth$/)
  assert.match(lines[5], /^ 6\. sausage +12\. yellow$/)
})
