import { strict as assert } from 'node:assert'
import { test } from 'node:test'
import { REPORT_EMAIL, REPORT_REASONS, contactMailto, nativeReportMail, platformLabel, reportBody, reportMailto } from '../src/lib/report.ts'

const decode = (url: string) => {
  const [addr, query] = url.slice('mailto:'.length).split('?')
  const params = Object.fromEntries(query.split('&').map((kv) => {
    const [k, v] = kv.split('=')
    return [k, decodeURIComponent(v)]
  }))
  return { addr, ...params } as { addr: string; subject: string; body?: string }
}

test('a report is a mailto to the one support address, subject names the reason', () => {
  const url = reportMailto({ reason: 'spam', subject: 'person', personName: 'Mong', personId: 'abc123' })
  const m = decode(url)
  assert.equal(m.addr, REPORT_EMAIL)
  assert.equal(REPORT_EMAIL, 'imamiller64@gmail.com')
  assert.equal(m.subject, 'DropBeam report: Spam or unwanted messages')
  assert.match(m.body!, /From: Mong \(DropBeam id abc123\)/)
  assert.match(m.body!, /Reported: a person/)
  assert.match(m.body!, /Blocked: no/)
  // Line breaks are CRLF per RFC 6068, and nothing unescaped breaks the URL.
  assert.ok(m.body!.includes('\r\n'))
  assert.ok(!/[\s#]/.test(url.split('?')[1]), 'query is fully percent-encoded')
})

test('the message text is included only when chosen, quoted and clipped', () => {
  const without = reportBody({ reason: 'harassment', subject: 'message', personName: 'X', messageText: null })
  assert.ok(!without.includes('Message text'))
  const long = 'a'.repeat(3000)
  const body = reportBody({ reason: 'harassment', subject: 'message', personName: 'X', messageText: `line one\n${long}`, messageTs: 0 })
  assert.match(body, /Message text:\n> line one\n> a+…/)
  assert.ok(body.length < 2300, 'long quotes are clipped')
})

test('a file report carries names only, never paths or contents', () => {
  const body = reportBody({ reason: 'sexual', subject: 'file', personName: 'X', fileNames: ['a.jpg', 'b.mov'], blocked: true })
  assert.match(body, /File names: a\.jpg, b\.mov/)
  assert.match(body, /Reported: a file/)
  assert.match(body, /Blocked: yes/)
})

test('notes, version and platform land in the body; unknown reasons fall back to their id', () => {
  const body = reportBody({ reason: 'weird', subject: 'person', personName: '', notes: '  kept spamming  ', appVersion: '0.53.0', platform: 'iOS' })
  assert.match(body, /^Reason: weird/)
  assert.match(body, /From: Unknown/)
  assert.match(body, /Details:\nkept spamming/)
  assert.match(body, /App: DropBeam 0\.53\.0 \(iOS\)/)
})

test('every reason has a label and a unique id', () => {
  assert.equal(new Set(REPORT_REASONS.map((r) => r.id)).size, REPORT_REASONS.length)
  assert.ok(REPORT_REASONS.every((r) => r.label.length > 3))
})

test('contact mail and platform labels', () => {
  const m = decode(contactMailto('1.2.3', 'macOS'))
  assert.equal(m.addr, REPORT_EMAIL)
  assert.match(m.body!, /DropBeam 1\.2\.3 \(macOS\)/)
  assert.equal(decode(contactMailto()).body, undefined)
  assert.equal(platformLabel('Mozilla/5.0 (iPhone; CPU iPhone OS 18_0 like Mac OS X)'), 'iOS')
  assert.equal(platformLabel('Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7)'), 'macOS')
  assert.equal(platformLabel('Mozilla/5.0 (Windows NT 10.0; Win64; x64)'), 'Windows')
  assert.equal(platformLabel('Mozilla/5.0 (X11; Linux x86_64)'), 'Linux')
})

test('the iOS report sheet includes the message text only when the switch is on', () => {
  const friend = { name: 'Spam', endpointId: 'e1' }
  const message = { kind: 'text', text: 'rude words', files: [], ts: 1_700_000_000_000 }
  const off = nativeReportMail({ friend, message, reason: 'harassment', includeText: false, notes: '', alsoBlock: true })
  assert.ok(!off.body.includes('rude words'))
  assert.match(off.body, /Reported: a message/)
  assert.match(off.body, /Blocked: yes/)
  assert.match(off.body, /\(iOS\)/)
  const on = nativeReportMail({ friend, message, reason: 'harassment', includeText: true, notes: 'x', alsoBlock: false })
  assert.match(on.body, /> rude words/)
  assert.equal(on.to, REPORT_EMAIL)
  assert.equal(decode(on.url).body, on.body.replace(/\n/g, '\r\n'))
  // A file message: names only; an unsent (deleted) message quotes nothing.
  const file = nativeReportMail({ friend, message: { kind: 'file', text: '', files: ['x.jpg'], ts: 1 }, reason: 'sexual', includeText: true, notes: '', alsoBlock: false })
  assert.match(file.body, /File name: x\.jpg/)
  assert.match(file.body, /Reported: a file/)
  const gone = nativeReportMail({ friend, message: { ...message, deleted: true }, reason: 'spam', includeText: true, notes: '', alsoBlock: false })
  assert.ok(!gone.body.includes('rude words'))
  const person = nativeReportMail({ friend, reason: 'spam', includeText: true, notes: '', alsoBlock: false })
  assert.match(person.body, /Reported: a person/)
})
