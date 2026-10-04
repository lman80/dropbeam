/**
 * DropBeam push relay — a tiny Cloudflare Worker (`dropbeam-push`).
 *
 * Transfer Servers (someone's always-on DropBeam) call this when they're holding
 * something for an iPhone that's asleep. The Worker holds the APNs key and turns
 * the call into a notification; it never sees message text (the sender sealed the
 * preview to the phone's own key) or a readable device token (the phone sealed its
 * token to THIS Worker's key, bound to one server).
 *
 *   POST /push    {v, server, sealed_token, collapse, payload, ts, sig}
 *                 sig = Ed25519 by `server` (its DropBeam endpoint key) over
 *                 "dropbeam-push-v1\n" + [v, server, sealed_token, collapse, payload, ts].join("\n")
 *   GET  /health  {configured}
 *
 * Secrets: APNS_KEY_P8 (the .p8 contents), APNS_KEY_ID, APNS_TEAM_ID,
 *          WORKER_SEAL_PRIV (X25519 private JWK from genkey.js).
 * Vars:    APNS_TOPIC (default com.ashtonmiller.dropbeam).
 * Rate limits (wrangler.toml `[[ratelimits]]`, no KV — KV's daily write quota
 * was a kill switch anyone could trip). Every key is derived by the Worker,
 * never chosen by the caller:
 *   RL_IP     per client network, checked before any crypto: a full IPv4
 *             address, or an IPv6 /64 (one subscriber's allocation — keying the
 *             full v6 address would let one host rotate through 2^64 buckets).
 *             It only ever blocks that one network, so it isn't a global lever.
 *   RL_SERVER per verified server key, RL_TOKEN per SHA-256 of the decrypted
 *             APNs token (= per phone).
 *   RL_GLOBAL the whole relay — charged LAST, only for requests that passed the
 *             signature check, opened a token sealed for that server, and
 *             passed their per-server / per-phone limits. Junk and forged
 *             requests never touch it, so they can't use it to deny push to
 *             everyone. (Residual: someone holding many genuinely sealed
 *             tokens across many server keys could still fill it; each pair
 *             is capped by RL_SERVER/RL_TOKEN, so it takes real phones.)
 * Each binding is optional; a missing one falls back to a per-isolate in-memory
 * counter, and an hourly per-isolate budget per phone/server backs the
 * per-minute bindings.
 *
 * Logs only APNs status codes (for `wrangler tail`). See docs/PUSH-SETUP.md.
 */

const PER_TOKEN_HOUR = 60 // per phone (servers also coalesce to one per 30s)
const PER_SERVER_HOUR = 500
// In-memory fallbacks for the per-minute bindings (same numbers as wrangler.toml).
const FALLBACK_MINUTE = { RL_IP: 30, RL_GLOBAL: 600, RL_SERVER: 60, RL_TOKEN: 10 }
const MAX_BODY = 8 * 1024
const MAX_PAYLOAD = 3000
const enc = new TextEncoder()

export default {
  async fetch(request, env) {
    const url = new URL(request.url)
    if (url.pathname === '/health') {
      return json({ ok: true, configured: configured(env) })
    }
    if (request.method === 'POST' && url.pathname === '/push') {
      try {
        // Cheap gate first: per client network (IPv4 address / IPv6 /64), before
        // reading or verifying anything. The GLOBAL limit is charged inside
        // push(), only once the request has fully verified.
        const net = ipKey(request.headers.get('cf-connecting-ip'))
        if (!(await allowMinute(env, 'RL_IP', `ip:${net}`))) {
          return json({ ok: false, reason: 'rate' }, 429)
        }
        return await push(request, env)
      } catch (e) {
        return json({ ok: false, reason: 'error' }, 500)
      }
    }
    return new Response('DropBeam push relay', { headers: { 'content-type': 'text/plain' } })
  },
}

function configured(env) {
  return !!(env.APNS_KEY_P8 && env.APNS_KEY_ID && env.APNS_TEAM_ID && env.WORKER_SEAL_PRIV)
}

function json(body, status = 200) {
  return new Response(JSON.stringify(body), { status, headers: { 'content-type': 'application/json' } })
}

const b64 = (s) => Uint8Array.from(atob(s), (c) => c.charCodeAt(0))
const hexBytes = (h) => /^[0-9a-f]{64}$/i.test(h) ? Uint8Array.from(h.match(/../g), (x) => parseInt(x, 16)) : null
const b64url = (bytes) => btoa(String.fromCharCode(...new Uint8Array(bytes))).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')

/** The body as text, or null past `max` bytes — counted as they stream in, so a
 * chunked upload or a lying Content-Length can't get past the cap. */
async function readCapped(request, max) {
  const declared = Number(request.headers.get('content-length') || 0)
  if (declared > max) return null
  if (!request.body) return ''
  const reader = request.body.getReader()
  const chunks = []
  let total = 0
  for (;;) {
    const { done, value } = await reader.read()
    if (done) break
    total += value.byteLength
    if (total > max) {
      try { await reader.cancel() } catch {}
      return null
    }
    chunks.push(value)
  }
  const all = new Uint8Array(total)
  let at = 0
  for (const c of chunks) { all.set(c, at); at += c.byteLength }
  return new TextDecoder().decode(all)
}

async function push(request, env) {
  const raw = await readCapped(request, MAX_BODY)
  if (raw === null) return json({ ok: false, reason: 'too_big' }, 413)
  let b
  try { b = JSON.parse(raw) } catch { return json({ ok: false, reason: 'invalid' }, 400) }
  if (!b || b.v !== 1 || typeof b.server !== 'string' || typeof b.sealed_token !== 'string' || typeof b.ts !== 'number' || typeof b.sig !== 'string'
    || (b.collapse != null && typeof b.collapse !== 'string') || (b.payload != null && typeof b.payload !== 'string')
    || b.sealed_token.length > 2048 || b.sig.length > 128) {
    return json({ ok: false, reason: 'invalid' }, 400)
  }
  if (Math.abs(Date.now() - b.ts) > 10 * 60 * 1000) return json({ ok: false, reason: 'stale' }, 400)
  const fields = [b.v, b.server, b.sealed_token, b.collapse ?? '', b.payload ?? '', b.ts]
  // 1) The call really comes from that server.
  const serverKey = hexBytes(b.server)
  if (!serverKey) return json({ ok: false, reason: 'invalid' }, 400)
  const key = await crypto.subtle.importKey('raw', serverKey, { name: 'Ed25519' }, false, ['verify'])
  const msg = enc.encode('dropbeam-push-v1\n' + fields.join('\n'))
  let good = false
  try { good = await crypto.subtle.verify({ name: 'Ed25519' }, key, b64(b.sig), msg) } catch { good = false }
  if (!good) return json({ ok: false, reason: 'signature' }, 401)
  if (!configured(env)) return json({ ok: false, reason: 'not_configured' }, 503)
  // 2) The token was sealed for THIS server.
  let tok
  try { tok = await openToken(env, b.sealed_token) } catch { return json({ ok: false, reason: 'token' }, 400) }
  if (tok.allowed_server !== b.server) return json({ ok: false, reason: 'token' }, 403)
  if (typeof tok.exp !== 'number' || tok.exp < Date.now()) return json({ ok: false, gone: true, reason: 'expired' }, 410)
  if (!/^[0-9a-f]{64,200}$/i.test(tok.token || '')) return json({ ok: false, reason: 'token' }, 400)
  // 3) Rate limits, keyed on what the Worker verified: the signing server's key
  //    and the phone's real APNs token (hashed), never on caller-chosen strings.
  const phone = await sha256hex(tok.token.toLowerCase())
  if (!(await allowMinute(env, 'RL_SERVER', `s:${b.server}`)) || !(await allowMinute(env, 'RL_TOKEN', `t:${phone}`))
    || !allowHour(`s:${b.server}`, PER_SERVER_HOUR) || !allowHour(`t:${phone}`, PER_TOKEN_HOUR)) {
    return json({ ok: false, reason: 'rate' }, 429)
  }
  //    Only now — signed, sealed for this server, within its own limits — does
  //    the request count against the relay-wide budget.
  if (!(await allowMinute(env, 'RL_GLOBAL', 'all'))) return json({ ok: false, reason: 'rate' }, 429)
  // 4) APNs.
  const payload = typeof b.payload === 'string' && b.payload.length <= MAX_PAYLOAD ? b.payload : ''
  const body = {
    aps: { alert: { title: 'DropBeam', body: 'New message' }, 'mutable-content': 1, sound: 'default', 'thread-id': String(b.collapse || '').slice(0, 64) },
    e: payload,
  }
  const host = tok.env === 'sandbox' ? 'api.sandbox.push.apple.com' : 'api.push.apple.com'
  const res = await fetch(`https://${host}/3/device/${tok.token}`, {
    method: 'POST',
    headers: {
      authorization: `bearer ${await jwt(env)}`,
      'apns-topic': env.APNS_TOPIC || tok.bundle || 'com.ashtonmiller.dropbeam',
      'apns-push-type': 'alert',
      'apns-priority': '10',
    },
    body: JSON.stringify(body),
  })
  // Status only (no token, no payload) so `wrangler tail` can confirm delivery.
  console.log(`apns ${res.status} ${tok.env === 'sandbox' ? 'sandbox' : 'prod'}`)
  if (res.status === 410 || res.status === 400) {
    const r = await res.json().catch(() => ({}))
    console.log(`apns reason ${String(r.reason || '').slice(0, 40)}`)
    if (res.status === 410 || r.reason === 'BadDeviceToken' || r.reason === 'Unregistered') return json({ ok: false, gone: true })
    return json({ ok: false, reason: r.reason || 'apns' }, 502)
  }
  return json({ ok: res.ok, status: res.status })
}

/** eph_pub(32) || nonce(12) || AES-256-GCM(ct+tag); key = HKDF-SHA256(X25519(worker, eph), eph_pub || worker_pub, "dropbeam-push-token-v1"). */
async function openToken(env, sealed) {
  const raw = b64(sealed)
  const eph = raw.slice(0, 32), nonce = raw.slice(32, 44), ct = raw.slice(44)
  const jwk = JSON.parse(env.WORKER_SEAL_PRIV)
  const priv = await crypto.subtle.importKey('jwk', jwk, { name: 'X25519' }, false, ['deriveBits'])
  const pubKey = await crypto.subtle.importKey('raw', eph, { name: 'X25519' }, false, [])
  const ss = await crypto.subtle.deriveBits({ name: 'X25519', public: pubKey }, priv, 256)
  const workerPub = Uint8Array.from(atob(jwk.x.replace(/-/g, '+').replace(/_/g, '/') + '='.repeat((4 - jwk.x.length % 4) % 4)), (c) => c.charCodeAt(0))
  const salt = new Uint8Array([...eph, ...workerPub])
  const ikm = await crypto.subtle.importKey('raw', ss, 'HKDF', false, ['deriveKey'])
  const aes = await crypto.subtle.deriveKey({ name: 'HKDF', hash: 'SHA-256', salt, info: enc.encode('dropbeam-push-token-v1') }, ikm, { name: 'AES-GCM', length: 256 }, false, ['decrypt'])
  const pt = await crypto.subtle.decrypt({ name: 'AES-GCM', iv: nonce }, aes, ct)
  return JSON.parse(new TextDecoder().decode(pt))
}

let cachedJwt = null
async function jwt(env) {
  const now = Math.floor(Date.now() / 1000)
  if (cachedJwt && now - cachedJwt.iat < 50 * 60) return cachedJwt.token
  const pem = env.APNS_KEY_P8.replace(/-----[^-]+-----/g, '').replace(/\s+/g, '')
  const key = await crypto.subtle.importKey('pkcs8', b64(pem), { name: 'ECDSA', namedCurve: 'P-256' }, false, ['sign'])
  const head = b64url(enc.encode(JSON.stringify({ alg: 'ES256', kid: env.APNS_KEY_ID })))
  const claims = b64url(enc.encode(JSON.stringify({ iss: env.APNS_TEAM_ID, iat: now })))
  const sig = await crypto.subtle.sign({ name: 'ECDSA', hash: 'SHA-256' }, key, enc.encode(`${head}.${claims}`))
  cachedJwt = { iat: now, token: `${head}.${claims}.${b64url(sig)}` }
  return cachedJwt.token
}

/** Rate-limit key for a client address: a full IPv4 address (IPv4-mapped IPv6
 * counts as IPv4), or the first 4 hextets (/64) of an IPv6 address, expanded and
 * lowercased so every spelling of one /64 shares a bucket. Unparseable input
 * collapses to one shared 'unknown' bucket (Cloudflare always sends the header). */
function ipKey(raw) {
  let s = String(raw || '').trim().toLowerCase()
  if (!s) return 'unknown'
  if (s.startsWith('[') && s.endsWith(']')) s = s.slice(1, -1)
  const pct = s.indexOf('%') // zone id
  if (pct >= 0) s = s.slice(0, pct)
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(s)) return validV4(s) ? `4:${s}` : 'unknown'
  if (!s.includes(':') || !/^[0-9a-f:.]+$/.test(s)) return 'unknown'
  // Trailing dotted IPv4 (e.g. ::ffff:1.2.3.4) → two hextets.
  const v4tail = s.match(/^(.*:)(\d{1,3}(?:\.\d{1,3}){3})$/)
  if (v4tail) {
    if (!validV4(v4tail[2])) return 'unknown'
    const o = v4tail[2].split('.').map(Number)
    s = v4tail[1] + ((o[0] << 8) | o[1]).toString(16) + ':' + ((o[2] << 8) | o[3]).toString(16)
  }
  const halves = s.split('::')
  if (halves.length > 2) return 'unknown'
  const part = (x) => (x === '' ? [] : x.split(':'))
  const head = part(halves[0])
  const tail = halves.length === 2 ? part(halves[1]) : []
  let groups
  if (halves.length === 2) {
    const fill = 8 - head.length - tail.length
    if (fill < 1) return 'unknown'
    groups = [...head, ...Array(fill).fill('0'), ...tail]
  } else {
    groups = head
  }
  if (groups.length !== 8 || groups.some((g) => !/^[0-9a-f]{1,4}$/.test(g))) return 'unknown'
  const h = groups.map((g) => parseInt(g, 16))
  // IPv4-mapped (::ffff:a.b.c.d) → the IPv4 address itself.
  if (h.slice(0, 5).every((x) => x === 0) && h[5] === 0xffff) {
    return `4:${h[6] >> 8}.${h[6] & 255}.${h[7] >> 8}.${h[7] & 255}`
  }
  return `6:${h.slice(0, 4).map((x) => x.toString(16)).join(':')}::/64`
}

function validV4(s) {
  return s.split('.').every((o) => Number(o) <= 255)
}

async function sha256hex(s) {
  const d = new Uint8Array(await crypto.subtle.digest('SHA-256', enc.encode(s)))
  return Array.from(d, (x) => x.toString(16).padStart(2, '0')).join('')
}

// Per-isolate counters (fallback + hourly budgets). Bounded so a flood of
// distinct keys can't grow memory without limit.
const memoryCounts = new Map()
function bump(bucket, max) {
  if (memoryCounts.size > 50_000) memoryCounts.clear()
  const n = memoryCounts.get(bucket) || 0
  if (n >= max) return false
  memoryCounts.set(bucket, n + 1)
  return true
}

function allowHour(key, perHour) {
  return bump(`h:${key}:${Math.floor(Date.now() / 3_600_000)}`, perHour)
}

/** Per-minute limit through the `name` rate-limit binding (per Cloudflare
 * location), or an in-memory counter when the binding isn't configured. */
async function allowMinute(env, name, key) {
  const rl = env && env[name]
  if (rl && typeof rl.limit === 'function') {
    try {
      const { success } = await rl.limit({ key })
      return success
    } catch {
      // Binding hiccup: fall through to the local counter rather than fail open/closed.
    }
  }
  return bump(`m:${name}:${key}:${Math.floor(Date.now() / 60_000)}`, FALLBACK_MINUTE[name] || 60)
}
