import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { nativeTabBarModel } from '../src/lib/nativeTabBarModel.ts'

const empty = { view: 'send', transfers: {}, chatUnread: {}, activeChatId: null }

test('native navigation maps all views, including locations and desktop-only folders', () => {
  for (const [view, index] of Object.entries({ send: 0, friends: 1, locations: 1, chat: 2, history: 3, settings: 4, folders: 0 })) {
    assert.equal(nativeTabBarModel({ ...empty, view }).index, index)
  }
})

test('native badges count only active states and sum all conversation unread counts', () => {
  const states = ['starting', 'waitingForPeer', 'connecting', 'waitingForAccept', 'transferring', 'paused', 'completed', 'failed', 'cancelled']
  const model = nativeTabBarModel({ ...empty,
    transfers: Object.fromEntries(states.map((state, i) => [i, { state }])),
    chatUnread: { alice: 99, bob: 4, carol: 0 },
  })
  assert.equal(model.sendBadge, 5)
  assert.equal(model.chatBadge, 103) // Rust formats this as 99+.
  assert.deepEqual(nativeTabBarModel(empty), { index: 0, sendBadge: 0, chatBadge: 0, hidden: false })
})

test('native bar hides for a conversation or keyboard and returns when both close', () => {
  assert.equal(nativeTabBarModel({ ...empty, view: 'chat', activeChatId: 'alice' }).hidden, true)
  assert.equal(nativeTabBarModel({ ...empty, activeChatId: 'alice' }).hidden, false)
  assert.equal(nativeTabBarModel({ ...empty, view: 'chat' }).hidden, false)
  assert.equal(nativeTabBarModel(empty, true).hidden, true)
  assert.equal(nativeTabBarModel(empty, false).hidden, false)
})
