import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { nativeReply, nativeChatSource, nativeTransfers, nativeThread } from '../src/lib/nativeChatBridge.ts'
import { changedSnapshots } from '../src/lib/nativeBridgeProtocol.ts'
import type { ChatMessage, TransferUpdate } from '../src/lib/api.ts'

const message = { id: 'm1', peerId: 'alice', fromMe: false, ts: 100, text: 'hello', reactions: [] } as unknown as ChatMessage

test('native reply IDs resolve authoritative messages and reject missing/deleted quotes', () => {
  assert.equal(nativeReply([message], 'm1'), message)
  assert.equal(nativeReply([message], undefined), undefined)
  assert.equal(nativeReply([message], null), undefined)
  assert.throws(() => nativeReply([message], { id: 'm1', text: 'forged' }))
  assert.throws(() => nativeReply([message], 'missing'))
  assert.throws(() => nativeReply([{ ...message, deleted: true }], 'm1'))
})
test('native attachment sources only accept explicit native pickers', () => {
  assert.equal(nativeChatSource({ source: 'photos' }), 'photos')
  assert.equal(nativeChatSource({ source: 'files' }), 'files')
  for (const source of ['camera', '', undefined, 1]) assert.throws(() => nativeChatSource({ source }))
})
test('native transfer snapshot preserves batch IDs and separates Send rows', () => {
  const regular = { id: 'engine-id', percent: 10 } as TransferUpdate
  const batch = { id: 'receive-id', percent: 42, chatTransfer: { completedPaths: { 'file:0:a.pdf': '/a.pdf' } } } as unknown as TransferUpdate
  const result = nativeTransfers([regular], { 'shared-id': batch })
  assert.equal(result[0].id, 'engine-id')
  assert.equal(result[0].chatOnly, false)
  assert.equal(result[1].id, 'shared-id')
  assert.equal(result[1].chatOnly, true)
  assert.equal(result[1].percent, 42)
  assert.deepEqual(result[1].chatTransfer?.completedPaths, { 'file:0:a.pdf': '/a.pdf' })
  assert.equal(batch.id, 'receive-id')
})
test('thread snapshots handle opening, switching and closing without cross-peer leakage', () => {
  assert.deepEqual(nativeThread('alice', { alice: [message] }), { friendId: 'alice', messages: [message] })
  assert.deepEqual(nativeThread('bob', { alice: [message] }), { friendId: 'bob', messages: [] })
  assert.equal(nativeThread(null, { alice: [message] }), null)
})
test('thread JSON diff detects edits, reactions and receipts without duplicate pushes', () => {
  const previous = new Map<string, string>()
  const snapshots = (m: ChatMessage) => ({ thread: nativeThread('alice', { alice: [m] }) })
  assert.equal(changedSnapshots(previous, snapshots(message)).length, 1)
  assert.equal(changedSnapshots(previous, snapshots({ ...message })).length, 0)
  for (const patch of [{ text: 'edited', edited: true }, { reactions: [{ emoji: '❤️', fromMe: true }] }, { status: 'read' as const }, { deleted: true }]) {
    assert.equal(changedSnapshots(previous, snapshots({ ...message, ...patch })).length, 1)
  }
})
