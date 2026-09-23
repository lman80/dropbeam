// Every DropBeam code that can be shared as text or a QR, in ONE place, so a
// scanned value, a pasted value and a typed value are all normalized the same
// way and routed to the same action. Pure (no store / DOM) so it's unit-tested.
//
// Formats (payload = base64url JSON, so it starts with "eyJ" in practice):
//   direct…          one-time Quick Send receive ticket   (iroh_net.rs TICKET_PREFIX)
//   dropbeam:…       permanent friend code                 (friends.rs USER_PREFIX)
//   dropbeamf1:…     legacy one-time friend invite         (friends.rs INVITE_PREFIX)
//   dropbeam1:…      shared-folder invite                  (pairing.rs INVITE_PREFIX)
//   dropbeamlink1:…  link-a-device code                    (link.rs PREFIX)
// The engine matches prefixes case-insensitively and needs the payload
// byte-for-byte (codes.rs), so we lowercase ONLY the prefix.

export type CodeKind = 'receive' | 'friend' | 'friendInvite' | 'folderInvite' | 'deviceLink'
export interface ParsedCode { kind: CodeKind; code: string }

// Longest first: "dropbeam:" must not swallow "dropbeamf1:" etc. (it can't — the
// colon differs — but order keeps it obviously correct).
const PREFIXES: readonly [string, CodeKind][] = [
  ['dropbeamlink1:', 'deviceLink'],
  ['dropbeamf1:', 'friendInvite'],
  ['dropbeam1:', 'folderInvite'],
  ['dropbeam:', 'friend'],
  ['direct', 'receive'],
]
const PAYLOAD = /^[A-Za-z0-9_-]+$/
// Codes embedded in a URL or a sentence ("Add me on DropBeam: dropbeam:eyJ…",
// "dropbeam://add?code=…", "https://…#dropbeam1:…"). "direct" is a normal word,
// so embedded it only counts when the JSON payload ("eyJ") follows directly.
const EMBEDDED = /(dropbeamlink1:|dropbeamf1:|dropbeam1:|dropbeam:)([A-Za-z0-9_-]{8,})|(direct)(eyJ[A-Za-z0-9_-]{8,})/i

function whole(text: string): ParsedCode | null {
  // Wrapped/indented pastes (chat apps break long codes) — payloads never
  // contain whitespace, so it's safe to drop it all for a whole-string match.
  // A sentence that merely STARTS with a prefix-like word ("directions to…")
  // must not collapse into a fake code, so with whitespace present the payload
  // has to look like real base64url JSON.
  const spaced = /\s/.test(text)
  const compact = text.replace(/\s+/g, '')
  const lower = compact.toLowerCase()
  for (const [prefix, kind] of PREFIXES) {
    if (!lower.startsWith(prefix)) continue
    const payload = compact.slice(prefix.length)
    if (!PAYLOAD.test(payload) || (spaced && !payload.startsWith('eyJ'))) return null
    if (kind === 'receive' && payload.length < 16) return null
    return { kind, code: prefix + payload }
  }
  return null
}

/** Normalize anything a user pasted, typed or scanned into a canonical code,
 *  or null if it isn't a DropBeam code. */
export function parseCode(raw: string | null | undefined): ParsedCode | null {
  let text = (raw ?? '').trim().replace(/^[\s"'`<([]+|[\s"'`>)\].,;!]+$/g, '')
  if (!text) return null
  const direct = whole(text)
  if (direct) return direct
  // Deep links / URLs: percent-decoded query/hash values carry the code.
  if (/%[0-9a-f]{2}/i.test(text)) {
    try { text = decodeURIComponent(text) } catch { /* keep as-is */ }
  }
  const m = EMBEDDED.exec(text)
  if (!m) return null
  const prefix = (m[1] ?? m[3]).toLowerCase()
  const payload = m[2] ?? m[4]
  return { kind: PREFIXES.find(([p]) => p === prefix)![1], code: prefix + payload }
}

/** The code string to hand the engine: canonical when recognized, else the
 *  trimmed input (so the engine's own error message still reaches the user). */
export function normalizeCode(raw: string): string {
  return parseCode(raw)?.code ?? raw.trim()
}

export const CODE_LABEL: Record<CodeKind, string> = {
  receive: 'a Quick Send code',
  friend: 'a friend code',
  friendInvite: 'a friend invite',
  folderInvite: 'a shared-folder invite',
  deviceLink: 'a device-linking code',
}

/** Where each kind of code is used — for "that's X, use it in Y" messages. */
const CODE_HOME: Record<CodeKind, string> = {
  receive: 'Send & Receive → “Have a code?”',
  friend: 'Friends → Add friend',
  friendInvite: 'Friends → Add friend',
  folderInvite: 'Shared Folders → Accept invite',
  deviceLink: 'Settings → Devices → Link a device',
}

/** Clear message for a code that isn't what this field takes (or isn't a
 *  DropBeam code at all). `expected` = the kinds this field accepts. */
export function wrongCodeMessage(expected: readonly CodeKind[], got: ParsedCode | null): string {
  const want = expected.map((k) => CODE_LABEL[k].replace(/^an? /, '')).filter((v, i, a) => a.indexOf(v) === i).join(' or ')
  if (!got) return `That isn’t a DropBeam code. Scan or paste the ${want} exactly as it was shared.`
  const label = CODE_LABEL[got.kind]
  return `That’s ${label}, not ${/^[aeiou]/i.test(want) ? 'an' : 'a'} ${want}. Use it in ${CODE_HOME[got.kind]}.`
}

/** What a GENERIC code field (e.g. "Have a code? Receive files") does with a
 *  code of each kind. Every DropBeam code has exactly one sensible action, so a
 *  friend code scanned into Receive adds the friend instead of erroring. */
export type CodeRoute =
  | { action: 'receive'; code: string }
  | { action: 'addFriend'; code: string }
  | { action: 'acceptFriendInvite'; code: string }
  | { action: 'acceptFolderInvite'; code: string }
  | { action: 'linkDevice'; code: string }
  | { action: 'invalid'; message: string }

export function routeCode(raw: string): CodeRoute {
  const parsed = parseCode(raw)
  if (!parsed) return { action: 'invalid', message: 'That isn’t a DropBeam code. Paste the full code the other person shared, or scan their QR code.' }
  switch (parsed.kind) {
    case 'receive': return { action: 'receive', code: parsed.code }
    case 'friend': return { action: 'addFriend', code: parsed.code }
    case 'friendInvite': return { action: 'acceptFriendInvite', code: parsed.code }
    case 'folderInvite': return { action: 'acceptFolderInvite', code: parsed.code }
    case 'deviceLink': return { action: 'linkDevice', code: parsed.code }
  }
}

/** QR sizing: long codes (Quick Send tickets carry the sender's addresses and
 *  run to several hundred chars) get lower error correction and more pixels so
 *  modules stay big enough for a phone camera across a desk. */
export function qrSpec(value: string, base = 200): { level: 'L' | 'M'; size: number } {
  const n = value.length
  if (n > 600) return { level: 'L', size: Math.round(base * 1.3) }
  if (n > 280) return { level: 'L', size: Math.round(base * 1.15) }
  return { level: 'M', size: base }
}
