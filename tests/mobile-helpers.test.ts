import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { presenceText, friendCodeKind } from '../src/mobile/helpers.ts'

test('mobile presence uses online state, known timestamps and honest offline fallback', () => {
  const now = 200_000_000
  assert.equal(presenceText({ status: 'online', lastSeen: null }, now), 'Online now')
  assert.equal(presenceText({ status: 'unknown', lastSeen: null }, now), 'Offline')
  assert.equal(presenceText({ status: 'offline', lastSeen: NaN }, now), 'Offline')
  assert.equal(presenceText({ status: 'offline', lastSeen: now - 5 * 60_000 }, now), 'Last seen 5 min ago')
  assert.equal(presenceText({ status: 'offline', lastSeen: now - 60 * 60_000 }, now), 'Last seen 1 hr ago')
  assert.equal(presenceText({ status: 'offline', lastSeen: now - 86_400_000 }, now), 'Last seen 1 day ago')
  assert.equal(presenceText({ status: 'offline', lastSeen: 0 }, now), 'Last seen 2 days ago')
  assert.equal(presenceText({ status: 'offline', lastSeen: now + 1_000 }, now), 'Last seen 1 min ago')
})
test('mobile add friend dispatch trims and recognizes only anchored supported prefixes', () => {
  assert.equal(friendCodeKind(' \nDROPBEAMF1:invite\n'), 'invite')
  assert.equal(friendCodeKind(' dropbeam:permanent '), 'permanent')
  assert.equal(friendCodeKind('DropBeam:permanent'), 'permanent')
  for (const code of ['', '123456', 'https://dropbeam:example', 'prefix dropbeamf1:x', 'dropbeamlink1:device', 'dropbeamf2:x']) {
    assert.equal(friendCodeKind(code), null)
  }
})
