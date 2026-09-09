import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import type { TransferUpdate } from '../src/lib/api.ts'
import { completedChatItems, chatTransferUpdate, loadChatTransfers, saveChatTransfers, pruneChatTransfers, CHAT_ORPHAN_TTL } from '../src/lib/chatTransfer.ts'

const update = (patch: Partial<TransferUpdate> = {}): TransferUpdate => ({
  id: 'local-transfer', direction: 'receive', state: 'transferring',
  fileNames: ['a', 'b'], bytesDone: 25, bytesTotal: 50, percent: 50, speedBps: 10, etaSeconds: 2.5,
  locality: 'direct', connDetail: { path: 'direct', rttMs: 5, relay: null, upgrading: false },
  chatTransfer: { id: 'shared', attempt: 1, offset: 0, total: 100, last: false, batchState: 'transferring', bytesDone: 25 },
  ...patch,
} as TransferUpdate)
const linked = (patch: Partial<NonNullable<TransferUpdate['chatTransfer']>> = {}, fields: Partial<TransferUpdate> = {}) =>
  update({ ...fields, chatTransfer: { ...update().chatTransfer!, ...patch } })

test('a bare final push cannot certify any bytes or whole-batch completion', () => {
  const spoof = chatTransferUpdate(linked({ last: true, batchState: undefined, bytesDone: undefined }, { state: 'completed', bytesDone: 0, bytesTotal: 0 }))
  assert.equal(spoof.bytesDone, 0)
  assert.equal(spoof.percent, 0)
  assert.notEqual(spoof.state, 'completed')
  const incomplete = chatTransferUpdate(linked({ last: true, batchState: 'completed', bytesDone: 0 }))
  assert.notEqual(incomplete.state, 'completed')
  const between = chatTransferUpdate(linked({ bytesDone: 50 }, { state: 'completed' }))
  assert.equal(between.state, 'transferring')
  assert.equal(between.percent, 50)
  const done = chatTransferUpdate(linked({ bytesDone: 100, batchState: 'completed' }), between)
  assert.equal(done.state, 'completed')
  assert.equal(done.percent, 100)
})

test('terminal states latch within an attempt; stale attempts cannot overwrite retries', () => {
  for (const state of ['completed', 'failed', 'canceled'] as const) {
    const terminal = chatTransferUpdate(linked({ batchState: state, bytesDone: 100 }))
    assert.equal(chatTransferUpdate(update(), terminal), terminal)
    const retry = chatTransferUpdate(linked({ attempt: 2, bytesDone: 0 }), terminal)
    assert.equal(retry.chatTransfer?.attempt, 2)
    assert.equal(retry.state, 'transferring')
    assert.equal(chatTransferUpdate(linked({ batchState: 'failed' }), retry), retry)
  }
})

test('pruning retains linked active entries plus 200 recent terminal/orphan entries, in place', () => {
  const now = Date.now()
  const map = Object.fromEntries(Array.from({ length: 350 }, (_, i) => [`done${i}`, { ...update({ state: 'completed' }), updatedAt: now + i }]))
  map.active = { ...update(), updatedAt: 0 }
  map.orphan = { ...update(), updatedAt: now - CHAT_ORPHAN_TTL }
  const identity = map
  pruneChatTransfers(map, new Set(['active', ...Object.keys(map).filter(k => k.startsWith('done'))]), now)
  assert.equal(map, identity)
  assert.ok(map.active)
  assert.equal(map.orphan, undefined)
  assert.equal(Object.keys(map).length, 201)
  assert.ok(map.done349)
  assert.equal(map.done0, undefined)
})

function storage() {
  const data = new Map<string, string>()
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => data.set(key, value),
  } })
  return data
}

test('load rejects null, arrays, primitives and invalid entries; normalizes valid entries', () => {
  const data = storage()
  for (const malformed of ['null', '[]', '1', '"bad"', '{', '{"x":null}', '{"x":[]}']) {
    data.set('dropbeam-chat-transfer-outcomes', malformed)
    assert.equal(Object.keys(loadChatTransfers()).length, 0)
  }
  saveChatTransfers({ shared: linked({}, { fileNames: null as unknown as string[], connDetail: [] as never }) })
  const loaded = loadChatTransfers().shared
  assert.deepEqual(loaded.fileNames, [])
  assert.equal(loaded.connDetail, null)
  assert.equal(loaded.state, 'failed')
  Reflect.deleteProperty(globalThis, 'localStorage')
})

test('retry transition replaces persisted failure and an engine update confirms restored attempt', () => {
  storage()
  const old = chatTransferUpdate(linked({ batchState: 'failed' }))
  saveChatTransfers({ shared: old })
  const restoredFailure = loadChatTransfers().shared
  assert.equal(restoredFailure.unconfirmed, false)
  assert.equal(chatTransferUpdate(update(), restoredFailure), restoredFailure)
  const retry = chatTransferUpdate(linked({ attempt: 2, bytesDone: 0 }), old)
  saveChatTransfers({ shared: retry })
  const loaded = loadChatTransfers().shared
  assert.equal(loaded.chatTransfer?.attempt, 2)
  assert.equal(loaded.state, 'failed')
  assert.equal(loaded.unconfirmed, true)
  assert.equal(chatTransferUpdate(linked({ attempt: 1 }), loaded), loaded)
  saveChatTransfers({ shared: loaded })
  assert.equal(loadChatTransfers().shared.unconfirmed, true)
  const confirmed = chatTransferUpdate(linked({ attempt: 2, batchState: 'completed', bytesDone: 100 }), loaded)
  assert.equal(confirmed.state, 'completed')
  Reflect.deleteProperty(globalThis, 'localStorage')
})


test('orphan timeout is measured from first observation, even with continued ticks', () => {
  const now = Date.now()
  const previous = { ...chatTransferUpdate(update()), firstSeen: now - CHAT_ORPHAN_TTL, updatedAt: now - 1 }
  const map = { shared: chatTransferUpdate(update(), previous) }
  pruneChatTransfers(map, new Set(), now)
  assert.equal(Object.keys(map).length, 0)
})

test('split transfer checksums survive aggregation and reload, and reset on retry', () => {
  storage()
  const row = { name: 'a', size: 50, algorithm: 'SHA-256 / DropBeam blocks v1 (4 MiB)', digest: 'a'.repeat(64), peerDigest: 'a'.repeat(64), verified: true }
  const first = chatTransferUpdate(linked({ bytesDone: 50 }, { integrity: [row], state: 'completed' }))
  const second = chatTransferUpdate(linked({ bytesDone: 100, batchState: 'completed' }, { integrity: [{ ...row, name: 'b' }] }), first)
  assert.deepEqual(second.integrity?.map(r => r.name), ['a', 'b'])
  saveChatTransfers({ shared: second })
  assert.deepEqual(loadChatTransfers().shared.integrity, second.integrity)
  const retry = chatTransferUpdate(linked({ attempt: 2, bytesDone: 0 }), second)
  assert.deepEqual(retry.integrity, [])
  Reflect.deleteProperty(globalThis, 'localStorage')
})

test('a mismatched digest and retry message persist in the chat card', () => {
  storage()
  const row = { name: 'a', size: 50, algorithm: 'SHA-256 / DropBeam blocks v1 (4 MiB)', digest: 'a'.repeat(64), peerDigest: 'b'.repeat(64), verified: false }
  const failed = chatTransferUpdate(linked({ batchState: 'failed' }, { integrity: [row], state: 'failed', error: 'Verification failed — retry' }))
  saveChatTransfers({ shared: failed })
  assert.deepEqual(loadChatTransfers().shared.integrity, [row])
  assert.equal(loadChatTransfers().shared.error, 'Verification failed — retry')
  Reflect.deleteProperty(globalThis, 'localStorage')
})

test('duplicate-name completed paths remain independently openable after reload', () => {
  storage()
  const t = chatTransferUpdate(linked({ batchState: 'completed', bytesDone: 100,
    completedPaths: { 'file:0:same.bin': '/saved/same.bin', 'file:1:same.bin': '/saved/same (1).bin' },
  }))
  saveChatTransfers({ shared: t })
  const items = completedChatItems(loadChatTransfers().shared.chatTransfer?.completedPaths)
  assert.deepEqual(items.map(i => i.name), ['same.bin', 'same.bin'])
  assert.deepEqual(items.map(i => i.path), ['/saved/same.bin', '/saved/same (1).bin'])
  assert.notEqual(items[0].key, items[1].key)
  Reflect.deleteProperty(globalThis, 'localStorage')
})
