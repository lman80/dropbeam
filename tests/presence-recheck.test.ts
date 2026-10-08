import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import type { FolderStatus } from '../src/lib/api.ts'
import { claimPresenceChecks, friendPresence, resetPresenceChecks } from '../src/lib/presence.ts'

// friendPresence compares against the real clock, so anchor the fake one to it.
const NOW = Date.now()
const friends = [
  { id: 'sam', name: 'Sam', endpointId: 'eid-sam' },
  { id: 'mong', name: 'Mong', endpointId: 'eid-mong' },
  { id: 'old', name: 'Old Pairing', endpointId: null },
]
const noFolders: Record<string, FolderStatus> = {}

test('a friend who has gone quiet is claimed for an active re-check, an online one is not', () => {
  resetPresenceChecks()
  // Sam was seen 10 minutes ago — past the 2-minute online window, so stale.
  const seen = { sam: NOW - 600_000, mong: NOW - 10_000 }
  assert.equal(friendPresence('Sam', seen, noFolders).status, 'offline')
  assert.equal(friendPresence('Mong', seen, noFolders).status, 'online')
  // "old" has no device address: there is nothing to dial, so never claimed.
  assert.deepEqual(claimPresenceChecks(friends, seen, noFolders, NOW), ['sam'])
})

test('claiming is rate limited so opening several views does not become a dial storm', () => {
  resetPresenceChecks()
  const seen = {}
  assert.deepEqual(claimPresenceChecks(friends, seen, noFolders, NOW), ['sam', 'mong'])
  // Friends, then the Send sheet, then Locations, all within the cooldown.
  assert.deepEqual(claimPresenceChecks(friends, seen, noFolders, NOW + 1), [])
  assert.deepEqual(claimPresenceChecks(friends, seen, noFolders, NOW + 19_999), [])
  assert.deepEqual(claimPresenceChecks(friends, seen, noFolders, NOW + 20_000), ['sam', 'mong'])
})

test('a friend a shared folder reports as online is left alone; one it reports offline is checked', () => {
  resetPresenceChecks()
  const statuses: Record<string, FolderStatus> = {
    a: { peerName: 'Sam', peerOnline: true } as FolderStatus,
    b: { peerName: 'Mong', peerOnline: false } as FolderStatus,
  }
  assert.deepEqual(claimPresenceChecks(friends, {}, statuses, NOW), ['mong'])
})

test('a removed friend does not keep a cooldown slot forever', () => {
  resetPresenceChecks()
  assert.deepEqual(claimPresenceChecks(friends, {}, noFolders, NOW), ['sam', 'mong'])
  // Sam is removed; re-adding them later must be checkable immediately.
  const without = friends.filter((f) => f.id !== 'sam')
  claimPresenceChecks(without, {}, noFolders, NOW + 1)
  assert.deepEqual(claimPresenceChecks(friends, {}, noFolders, NOW + 2), ['sam'])
})

test('presence is kept per friend id, not per display name (#44)', () => {
  const a = { id: 'alex-1', name: 'Alex' }
  const b = { id: 'alex-2', name: 'Alex' }
  const seen = { 'alex-1': Date.now() }
  assert.equal(friendPresence(a, seen, noFolders).status, 'online')
  assert.equal(friendPresence(b, seen, noFolders).status, 'unknown', 'a namesake is not online just because the other Alex is')
  // A friend who renames keeps their presence.
  assert.equal(friendPresence({ id: 'alex-1', name: 'Alexander' }, seen, noFolders).status, 'online')
  // A pre-#44 name entry still counts for a friend with that name.
  assert.equal(friendPresence({ id: 'x', name: 'Sam' }, { sam: Date.now() - 600_000 }, noFolders).status, 'offline')
})
