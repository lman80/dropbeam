import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { contentSummary, deviceCodeKind, deviceCodeProblem, friendCodeProblem, isDeviceCode, linkedDetail, linkedTitle, linkErrorText, progressText } from '../src/lib/deviceLink.ts'
import { ownDeviceLabels } from '../src/lib/deviceIcons.ts'

test('device codes are told apart from friend codes and junk, in any case and with whitespace', () => {
  assert.equal(deviceCodeKind(' DropBeamLink1:abc\n'), 'link')
  assert.equal(deviceCodeKind('dropbeamjoin1:abc'), 'join')
  assert.equal(deviceCodeKind('dropbeam:abc'), 'friend')
  assert.equal(deviceCodeKind('DROPBEAMF1:abc'), 'friend')
  assert.equal(deviceCodeKind('https://example.com'), 'invalid')
  assert.ok(isDeviceCode('dropbeamjoin1:x') && isDeviceCode('dropbeamlink1:x') && !isDeviceCode('dropbeam:x'))
  // The device scanner says what was scanned instead, and keeps scanning.
  assert.equal(deviceCodeProblem('dropbeamlink1:x'), null)
  assert.match(deviceCodeProblem('dropbeam:x')!, /friend code/)
  assert.match(deviceCodeProblem('hello')!, /isn’t a DropBeam device code/)
  // …and the friend scanner points device codes at Settings → Devices.
  assert.match(friendCodeProblem('dropbeamjoin1:x')!, /Settings → Devices/)
  assert.equal(friendCodeProblem('dropbeam:x'), null)
})

test('progress and success read like a person wrote them', () => {
  assert.equal(contentSummary(7, 309), '7 friends and 309 messages')
  assert.equal(contentSummary(1, 1), '1 friend and 1 message')
  assert.equal(contentSummary(0, 12), '12 messages')
  assert.equal(contentSummary(0, 0), null)
  assert.equal(contentSummary(undefined, null), null)
  assert.equal(progressText(null), 'Connecting to your other device…')
  assert.equal(progressText({ stage: 'importing', friends: 7, messages: 309 }), 'Bringing over 7 friends and 309 messages…')
  assert.equal(progressText({ stage: 'sending', friends: 2, messages: 1500 }), `Sending 2 friends and ${(1500).toLocaleString()} messages…`)
  assert.equal(progressText({ stage: 'importing' }), 'Setting up this device…')
  assert.equal(progressText({ stage: 'waiting' }), 'Connected — waiting for your other device…')
  assert.equal(linkedTitle({ name: 'iPhone', device_kind: 'phone', device_os: 'ios' }), 'Linked with your iPhone')
  assert.equal(linkedTitle({ name: "Ashton's MacBook Pro", device_kind: 'laptop', device_os: 'macos' }), 'Linked with your Mac')
  assert.equal(linkedTitle({ name: 'Studio' }), 'Linked with Studio')
  assert.equal(linkedTitle(null), 'Your devices are linked')
  assert.equal(linkedDetail({ friends: 7, messages: 309 }), '7 friends and 309 messages are on both devices now. From here on your friends, chats, name and photo stay in sync.')
  assert.equal(linkedDetail({ friends: 0, messages: 0 }), 'From here on your friends, chats, name and photo stay in sync.')
  assert.equal(linkErrorText(new Error('Error: That code has expired.')), 'That code has expired.')
  assert.equal(linkErrorText('Couldn’t reach your other device.'), 'Couldn’t reach your other device.')
  assert.equal(linkErrorText(''), 'Linking didn’t work. Try again.')
})

test('own devices that would read the same get told apart', () => {
  // Two iPhones both report "iPhone": numbered, in a stable order.
  assert.deepEqual(ownDeviceLabels([{ id: 'b', name: 'iPhone', deviceKind: 'phone', deviceOs: 'ios' }, { id: 'a', name: 'iPhone', deviceKind: 'phone', deviceOs: 'ios' }, { id: 'm', name: 'Mac', deviceOs: 'macos' }]),
    { a: 'Your iPhone 1', b: 'Your iPhone 2', m: 'Your Mac' })
  // Distinct device names are used when they exist.
  assert.deepEqual(ownDeviceLabels([{ id: 'a', name: 'Work Mac', deviceOs: 'macos' }, { id: 'b', name: 'Home Mac', deviceOs: 'macos' }]), { a: 'Work Mac', b: 'Home Mac' })
})
