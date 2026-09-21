import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { linkify, trimTrailing, MAX_URL_LEN, type LinkSegment } from '../src/lib/linkify.ts'

const links = (text: string) => linkify(text).filter((s) => s.t === 'link').map((s) => s.v)
/** Segments must always re-join to the original text — nothing dropped or rewritten. */
const roundTrips = (text: string, segs: LinkSegment[] = linkify(text)) =>
  assert.equal(segs.map((s) => s.v).join(''), text)

test('plain text passes through as a single text segment', () => {
  const segs = linkify('just a normal message, nothing to click')
  assert.deepEqual(segs, [{ t: 'text', v: 'just a normal message, nothing to click' }])
  assert.deepEqual(linkify(''), [])
})

test('a lone url becomes one link segment', () => {
  assert.deepEqual(linkify('https://example.com/a'), [{ t: 'link', v: 'https://example.com/a' }])
  assert.deepEqual(links('HTTP://Example.COM'), ['HTTP://Example.COM'])
})

test('a url mid-sentence keeps its surrounding text and gives back the trailing period', () => {
  const text = 'see https://example.com/docs. thanks'
  assert.deepEqual(linkify(text), [
    { t: 'text', v: 'see ' },
    { t: 'link', v: 'https://example.com/docs' },
    { t: 'text', v: '. thanks' },
  ])
  roundTrips(text)
  for (const p of [',', ';', ':', '!', '?', '.']) {
    assert.deepEqual(links(`x https://a.dev/b${p} y`), ['https://a.dev/b'])
  }
})

test('parens: the wrapping pair is sentence punctuation, a balanced pair is part of the url', () => {
  assert.deepEqual(links('(https://example.com/a)'), ['https://example.com/a'])
  assert.deepEqual(links('read (https://en.wikipedia.org/wiki/Foo_(bar)) now'), [
    'https://en.wikipedia.org/wiki/Foo_(bar)',
  ])
  assert.deepEqual(links('[https://example.com/a]'), ['https://example.com/a'])
  roundTrips('read (https://en.wikipedia.org/wiki/Foo_(bar)) now')
  assert.deepEqual(trimTrailing('https://x.dev/a).'), { url: 'https://x.dev/a', trailing: ').' })
})

test('dangerous and relative schemes are never linked', () => {
  for (const bad of [
    'javascript:alert(1)',
    'JavaScript:alert(1)',
    'data:text/html;base64,PHNjcmlwdD4=',
    'file:///etc/passwd',
    'mailto:someone@example.com',
    'www.example.com',
    'ftp://example.com',
    'vbscript:msgbox(1)',
    '//example.com/a',
  ]) {
    assert.deepEqual(links(`look at ${bad} ok`), [], bad)
  }
  // A dangerous scheme wrapping an http url only ever links the http part.
  assert.deepEqual(links('javascript:https://evil.test/x'), ['https://evil.test/x'])
  // No scheme-less or hostless "url" slips through.
  assert.deepEqual(links('https:// nothing here'), [])
  assert.deepEqual(links('xhttps://example.com'), [])
})

test('multiple urls in one message each become their own link', () => {
  const text = 'a https://one.test/x b https://two.test/y c'
  assert.deepEqual(linkify(text), [
    { t: 'text', v: 'a ' },
    { t: 'link', v: 'https://one.test/x' },
    { t: 'text', v: ' b ' },
    { t: 'link', v: 'https://two.test/y' },
    { t: 'text', v: ' c' },
  ])
  roundTrips(text)
  assert.deepEqual(links('https://a.test\nhttps://b.test'), ['https://a.test', 'https://b.test'])
})

test('absurdly long urls stay plain text', () => {
  const long = `https://example.com/${'a'.repeat(MAX_URL_LEN)}`
  assert.deepEqual(links(long), [])
  roundTrips(long)
  assert.deepEqual(links(`https://example.com/${'a'.repeat(MAX_URL_LEN - 21)}`).length, 1)
})

test('html-looking text cannot extend a url past the markup boundary', () => {
  assert.deepEqual(links('<a href="https://example.com/a">x</a>'), ['https://example.com/a'])
})
