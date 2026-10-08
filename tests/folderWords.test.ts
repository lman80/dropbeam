import { test } from 'node:test'
import assert from 'node:assert/strict'
import { inviteMode, isBroadFolder, folderMode } from '../src/lib/folderWords.ts'

const enc = (o: object) => 'dropbeam1:' + Buffer.from(JSON.stringify(o)).toString('base64').replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')

test('invite mode is read from the code', () => {
  assert.equal(inviteMode(enc({ v: 1, tw: true, mir: true })), 'mirror')
  assert.equal(inviteMode(enc({ v: 1, tw: true })), 'twoway')
  assert.equal(inviteMode(enc({ v: 1, tw: false })), 'oneway')
  assert.equal(inviteMode('dropbeam1:%%%'), null)
  assert.equal(inviteMode('direct123'), null)
})

test('broad folders', () => {
  assert.equal(isBroadFolder('/Users/ann', '/Users/ann'), true)
  assert.equal(isBroadFolder('/Users/ann/Documents/', '/Users/ann'), true)
  assert.equal(isBroadFolder('/Users/ann/Documents/Trip', '/Users/ann'), false)
  assert.equal(isBroadFolder('C:\\Users\\Ann\\Desktop', 'C:\\Users\\Ann'), true)
  assert.equal(isBroadFolder('/', null), true)
  assert.equal(isBroadFolder('/Volumes/Photos/2024', null), false)
})

test('folder mode', () => {
  assert.equal(folderMode({ mirror: true, twoWay: true }), 'mirror')
  assert.equal(folderMode({ mirror: false, twoWay: false }), 'oneway')
})
