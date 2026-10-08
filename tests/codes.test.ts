import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { normalizeCode, parseCode, qrSpec, routeCode, wrongCodeMessage } from '../src/lib/codes.ts'

const J = 'eyJ2IjoxLCJlaWQiOiIwZjFlMmQzYzRiNWE2OTc4ODc5NmE1YjQiLCJuYW1lIjoiVGVzdCJ9'

test('every code kind is recognized and its prefix canonicalized, payload untouched', () => {
  const cases: [string, string, string][] = [
    [`direct${J}`, 'receive', `direct${J}`],
    [`DIRECT${J}`, 'receive', `direct${J}`],
    [`dropbeam:${J}`, 'friend', `dropbeam:${J}`],
    [`Dropbeam:${J}`, 'friend', `dropbeam:${J}`],
    [`DROPBEAMF1:${J}`, 'friendInvite', `dropbeamf1:${J}`],
    [`dropbeam1:${J}`, 'folderInvite', `dropbeam1:${J}`],
    [`DropBeamLink1:${J}`, 'deviceLink', `dropbeamlink1:${J}`],
  ]
  for (const [raw, kind, code] of cases) assert.deepEqual(parseCode(raw), { kind, code }, raw)
})

test('paste noise is stripped: whitespace, wrapping, quotes, trailing punctuation', () => {
  assert.equal(normalizeCode(`  dropbeam:${J}\n`), `dropbeam:${J}`)
  assert.equal(normalizeCode(`dropbeam:${J.slice(0, 20)}\n  ${J.slice(20)}`), `dropbeam:${J}`)
  assert.equal(normalizeCode(`"direct${J}".`), `direct${J}`)
  assert.equal(normalizeCode(`<dropbeam1:${J}>`), `dropbeam1:${J}`)
})

test('codes embedded in a sentence or a deep link / URL are extracted', () => {
  assert.deepEqual(parseCode(`Add me on DropBeam: dropbeam:${J} thanks!`), { kind: 'friend', code: `dropbeam:${J}` })
  assert.deepEqual(parseCode(`dropbeam://add?code=${encodeURIComponent(`dropbeam:${J}`)}`), { kind: 'friend', code: `dropbeam:${J}` })
  assert.deepEqual(parseCode(`https://example.com/r#direct${J}`), { kind: 'receive', code: `direct${J}` })
  assert.deepEqual(parseCode(`https://example.com/join?invite=DROPBEAM1%3A${J}`), { kind: 'folderInvite', code: `dropbeam1:${J}` })
})

test('ordinary text and other QR codes are not mistaken for codes', () => {
  for (const s of ['', '   ', 'hello', 'directions to my house', 'direct', 'directory', 'https://example.com', 'dropbeam:', 'dropbeam: is cool', 'WIFI:S:home;T:WPA;P:pw;;', 'direct message me']) {
    assert.equal(parseCode(s), null, s)
  }
  assert.equal(normalizeCode('  not a code '), 'not a code')
})

test('mock/short-but-valid codes still parse (dev preview)', () => {
  assert.equal(parseCode('dropbeam:MOCKpersonalcodewouldgohere0000')?.kind, 'friend')
  assert.equal(parseCode('direct1aBcDeFgH2jKlMnPqRsTuVwXyZ3456789aBcDeFgHjKmNpQ')?.kind, 'receive')
})

test('generic code fields route every kind to its one sensible action', () => {
  assert.deepEqual(routeCode(` direct${J} `), { action: 'receive', code: `direct${J}` })
  assert.deepEqual(routeCode(`dropbeam:${J}`), { action: 'addFriend', code: `dropbeam:${J}` })
  assert.deepEqual(routeCode(`dropbeamf1:${J}`), { action: 'acceptFriendInvite', code: `dropbeamf1:${J}` })
  assert.deepEqual(routeCode(`dropbeam1:${J}`), { action: 'acceptFolderInvite', code: `dropbeam1:${J}` })
  assert.deepEqual(routeCode(`dropbeamlink1:${J}`), { action: 'linkDevice', code: `dropbeamlink1:${J}` })
  assert.equal(routeCode('lol').action, 'invalid')
})

test('wrong-field messages name what was scanned and where it goes', () => {
  const m = wrongCodeMessage(['folderInvite'], parseCode(`dropbeam:${J}`))
  assert.match(m, /friend code/)
  assert.match(m, /shared-folder invite/)
  assert.match(m, /Friends → Add friend/)
  assert.match(wrongCodeMessage(['friend', 'friendInvite'], null), /isn’t a DropBeam code/)
})

test('long codes get a bigger, lower-ECC QR', () => {
  assert.deepEqual(qrSpec('x'.repeat(60)), { level: 'M', size: 200 })
  assert.equal(qrSpec('x'.repeat(500)).level, 'L')
  assert.ok(qrSpec('x'.repeat(900)).size > qrSpec('x'.repeat(500)).size)
})

test('a friend code still works when the "dropbeam:" part was deleted, or inside a whole invite message', () => {
  assert.deepEqual(parseCode(J), { kind: 'friend', code: `dropbeam:${J}` })
  assert.deepEqual(parseCode(`  ${J.slice(0, 30)}\n${J.slice(30)} `), { kind: 'friend', code: `dropbeam:${J}` })
  const message = `Add me on DropBeam — Test\nCopy this whole message, then open DropBeam → Add Friend.\n\ndropbeam:${J}\n`
  assert.deepEqual(parseCode(message), { kind: 'friend', code: `dropbeam:${J}` })
  assert.equal(routeCode(J).action, 'addFriend')
  // A bare payload that isn't a personal code (an invite has a secret) is not guessed at.
  const invite = Buffer.from(JSON.stringify({ v: 1, id: 'x', secret: 's', name: 'N', eid: 'e' })).toString('base64url')
  assert.equal(parseCode(invite), null)
  assert.equal(parseCode(Buffer.from('{"v":1}').toString('base64url')), null)
})

test('friendCodeName reads the name for "Add …?"', async () => {
  const { friendCodeName } = await import('../src/lib/codes.ts')
  assert.equal(friendCodeName(`dropbeam:${J}`), 'Test')
  assert.equal(friendCodeName(J), 'Test')
  const unicode = Buffer.from(JSON.stringify({ v: 1, eid: 'abc', name: 'Zoë 李' })).toString('base64url')
  assert.equal(friendCodeName(`dropbeam:${unicode}`), 'Zoë 李')
  assert.equal(friendCodeName(`direct${J}`), null)
})
