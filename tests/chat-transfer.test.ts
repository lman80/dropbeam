import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import type { TransferUpdate } from '../src/lib/api.ts'
import { chatTransferUpdate, loadChatTransfers, saveChatTransfers } from '../src/lib/chatTransfer.ts'

const update = (patch: Partial<TransferUpdate> = {}): TransferUpdate => ({
  id: 'local-transfer', direction: 'receive', state: 'transferring',
  bytesDone: 25, bytesTotal: 50, percent: 50, speedBps: 10, etaSeconds: 2.5,
  locality: 'direct', connDetail: { path: 'direct', rttMs: 5, relay: null, upgrading: false },
  chatTransfer: { id: 'shared', offset: 0, total: 100, last: false },
  ...patch,
} as TransferUpdate)

test('split pushes show whole-batch progress and only the last receipt completes chat', () => {
  const first = chatTransferUpdate(update())
  assert.equal(first.percent, 25)
  assert.equal(first.etaSeconds, 7.5)
  const between = chatTransferUpdate(update({ state: 'completed', bytesDone: 50 }), first)
  assert.equal(between.state, 'transferring')
  assert.equal(between.percent, 50)
  const final = chatTransferUpdate(update({ id: 'second-push', state: 'completed', bytesDone: 50,
    connDetail: null, locality: 'unknown',
    chatTransfer: { id: 'shared', offset: 50, total: 100, last: true } }), between)
  assert.equal(final.state, 'completed')
  assert.equal(final.bytesDone, 100)
  assert.equal(final.percent, 100)
  assert.equal(final.connDetail?.path, 'direct')
})

test('single outgoing transfer preserves the transfer-list speed and ETA', () => {
  const source = update({ direction: 'send', chatTransfer: { id: 'shared', offset: 0, total: 50, last: true } })
  const projected = chatTransferUpdate(source)
  for (const key of ['percent', 'bytesDone', 'bytesTotal', 'speedBps', 'etaSeconds'] as const) {
    assert.equal(projected[key], source[key])
  }
})

test('failure and retry keep shared identity while using the new transfer for Retry', () => {
  const failed = chatTransferUpdate(update({ state: 'failed', error: 'Connection lost' }))
  assert.equal(failed.error, 'Connection lost')
  const retry = chatTransferUpdate(update({ id: 'retry', state: 'connecting', bytesDone: 0 }), failed)
  assert.equal(retry.id, 'retry')
  assert.equal(retry.chatTransfer?.id, failed.chatTransfer?.id)
  assert.equal(retry.state, 'connecting')
})

test('terminal outcomes survive reload independently of dismissed transfer cards', () => {
  const data = new Map<string, string>()
  Object.defineProperty(globalThis, 'localStorage', { configurable: true, value: {
    getItem: (key: string) => data.get(key) ?? null,
    setItem: (key: string, value: string) => data.set(key, value),
  } })
  saveChatTransfers({ failed: update({ state: 'failed' }), live: update(),
    done: update({ state: 'completed' }) })
  assert.deepEqual(Object.keys(loadChatTransfers()).sort(), ['done', 'failed'])
  Reflect.deleteProperty(globalThis, 'localStorage')
})
