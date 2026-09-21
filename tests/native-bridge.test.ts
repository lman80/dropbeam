import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { changedSnapshots, dispatchNativeCall } from '../src/lib/nativeBridgeProtocol.ts'

test('native calls preserve correlation and JSON arguments including quotes and paths', async () => {
  const args = { name: '"; window.alert(1); //', paths: ['/tmp/a\nb.txt'] }
  assert.deepEqual(await dispatchNativeCall({ echo: a => a }, 12, 'echo', args), { id: 12, ok: true, value: args })
})
test('void results become JSON null and action errors become failed replies', async () => {
  assert.deepEqual(await dispatchNativeCall({ done: async () => {} }, 1, 'done', {}), { id: 1, ok: true, value: null })
  assert.deepEqual(await dispatchNativeCall({ fail: () => { throw new Error('Offline') } }, 2, 'fail', {}), { id: 2, ok: false, value: 'Offline' })
})
test('unknown and prototype handler names are rejected without calling them', async () => {
  for (const name of ['missing', 'constructor', 'toString', '__proto__']) {
    assert.equal((await dispatchNativeCall({}, 1, name, {})).ok, false)
  }
})
test('snapshots publish all keys initially and only changed JSON thereafter', () => {
  const previous = new Map<string, string>()
  const initial = { friends: [], transfers: [], settings: null, chatOverview: [], presence: {}, myDevice: null }
  assert.equal(changedSnapshots(previous, initial).length, 6)
  assert.deepEqual(changedSnapshots(previous, { ...initial }), [])
  assert.deepEqual(changedSnapshots(previous, { ...initial, presence: { alice: true } }), [{ key: 'presence', value: { alice: true } }])
  assert.deepEqual(changedSnapshots(previous, { ...initial }), [{ key: 'presence', value: {} }])
})
test('concurrent native calls keep their original IDs when replies finish out of order', async () => {
  let finish: (value: string) => void = () => {}
  const slow = dispatchNativeCall({ slow: () => new Promise<string>(resolve => { finish = resolve }) }, 7, 'slow', {})
  assert.deepEqual(await dispatchNativeCall({ fast: () => 'second' }, 8, 'fast', {}), { id: 8, ok: true, value: 'second' })
  finish('first')
  assert.deepEqual(await slow, { id: 7, ok: true, value: 'first' })
})


test('picker resnapshot waits for native reply and preserves staged files after resume', async () => {
  const { deliverNativeReply } = await import('../src/lib/nativeBridgeProtocol.ts')
  const events: string[] = []
  let complete!: () => void
  const delivery = new Promise<void>(resolve => { complete = resolve })
  const state = { chatDraftFiles: [] as string[] }
  const pending = deliverNativeReply({ id: 1, ok: true, value: ['/photo.heic'] }, async () => {
    events.push('reply'); await delivery
  }, () => { events.push('snapshot'); assert.deepEqual(state.chatDraftFiles, ['/photo.heic']) })
  assert.deepEqual(events, ['reply'])
  state.chatDraftFiles = ['/photo.heic']
  complete(); await pending
  assert.deepEqual(events, ['reply', 'snapshot'])
})

test('a lost picker reply does not run post-reply work', async () => {
  const { deliverNativeReply } = await import('../src/lib/nativeBridgeProtocol.ts')
  let pushed = false
  await assert.rejects(deliverNativeReply(null, async () => { throw new Error('webview closed') }, () => { pushed = true }))
  assert.equal(pushed, false)
})


test('native media routing invokes Photos and Files directly without a hidden web chooser', async () => {
  const { pickNativeMedia } = await import('../src/lib/nativeBridgeProtocol.ts')
  const commands: string[] = []
  const invoke = async (command: string) => {
    commands.push(command)
    return command === 'pick_photos' ? ['/current.heic'] : { paths: ['/document.pdf'] }
  }
  assert.deepEqual(await pickNativeMedia('photos', invoke), ['/current.heic'])
  assert.deepEqual(await pickNativeMedia('files', invoke), ['/document.pdf'])
  assert.deepEqual(commands, ['pick_photos', 'plugin:native-ui|pick_files'])
})

test('native media cancellation, provider errors and malformed paths remain distinguishable', async () => {
  const { pickNativeMedia } = await import('../src/lib/nativeBridgeProtocol.ts')
  assert.deepEqual(await pickNativeMedia('photos', async () => []), [])
  assert.deepEqual(await pickNativeMedia('files', async () => ({ paths: [] })), [])
  await assert.rejects(pickNativeMedia('photos', async () => { throw new Error('iCloud download failed') }), /iCloud download failed/)
  await assert.rejects(pickNativeMedia('files', async () => ({ paths: [42] })), /invalid file paths/)
})


test('native avatars preserve image formats and reject a selected video before updating the profile', async () => {
  const { nativeAvatarPath } = await import('../src/lib/nativeBridgeProtocol.ts')
  for (const path of ['/photo.HEIC', '/photo.heif', '/photo.png', '/photo.jpeg']) assert.equal(nativeAvatarPath(path), path)
  for (const path of ['/video.mov', '/video.mp4', '/unknown.img']) assert.throws(() => nativeAvatarPath(path), /Choose a photo/)
})
