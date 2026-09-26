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
 * KV:      PUSH_RL (rate limits; optional — without it limits are per-isolate).
 *
 * Logs nothing but counters. See docs/PUSH-SETUP.md.
 */

const PER_TOKEN_HOUR = 30
const PER_SERVER_HOUR = 500
const MAX_BODY = 8 * 1024
const enc = new TextEncoder()

export default {
  async fetch(request, env) {
    const url = new URL(request.url)
    if (url.pathname === '/health') {
      return json({ ok: true, configured: configured(env) })
    }
    if (request.method === 'POST' && url.pathname === '/push') {
      try {
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

async function push(request, env) {
  const raw = await request.text()
  if (raw.length > MAX_BODY) return json({ ok: false, reason: 'too_big' }, 413)
  let b
  try { b = JSON.parse(raw) } catch { return json({ ok: false, reason: 'invalid' }, 400) }
  const fields = [b.v, b.server, b.sealed_token, b.collapse ?? '', b.payload ?? '', b.ts]
  if (b.v !== 1 || typeof b.server !== 'string' || typeof b.sealed_token !== 'string' || typeof b.ts !== 'number' || typeof b.sig !== 'string') {
    return json({ ok: false, reason: 'invalid' }, 400)
  }
  if (Math.abs(Date.now() - b.ts) > 10 * 60 * 1000) return json({ ok: false, reason: 'stale' }, 400)
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
  // 3) Rate limits (per phone token, per server).
  if (!(await allow(env, `t:${tok.token.slice(0, 32)}`, PER_TOKEN_HOUR)) || !(await allow(env, `s:${b.server}`, PER_SERVER_HOUR))) {
    return json({ ok: false, reason: 'rate' }, 429)
  }
  // 4) APNs.
  const payload = typeof b.payload === 'string' && b.payload.length <= 3000 ? b.payload : ''
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
      ...(b.collapse ? { 'apns-collapse-id': String(b.collapse).slice(0, 64) } : {}),
    },
    body: JSON.stringify(body),
  })
  if (res.status === 410 || res.status === 400) {
    const r = await res.json().catch(() => ({}))
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

const memoryCounts = new Map()
async function allow(env, key, perHour) {
  const bucket = `${key}:${Math.floor(Date.now() / 3_600_000)}`
  if (env.PUSH_RL) {
    const n = parseInt((await env.PUSH_RL.get(bucket)) || '0', 10)
    if (n >= perHour) return false
    await env.PUSH_RL.put(bucket, String(n + 1), { expirationTtl: 7200 })
    return true
  }
  const n = memoryCounts.get(bucket) || 0
  if (n >= perHour) return false
  memoryCounts.set(bucket, n + 1)
  return true
}
