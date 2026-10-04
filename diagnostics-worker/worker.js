/**
 * DropBeam diagnostics collector — a tiny Cloudflare Worker.
 *
 * The app POSTs a redacted error/perf digest here (~once a day per device). This
 * Worker merges each device's digests into one rolling record in KV, and serves a
 * one-page dashboard you open daily to see every device's background issues.
 *
 * Two routes:
 *   POST /ingest            ← the app sends digests here (set this URL in DropBeam
 *                             Settings → Diagnostics endpoint).
 *   GET  /diag?key=SECRET   ← your private dashboard.
 *
 * Setup: see DIAGNOSTICS-SETUP.md. Needs one KV namespace bound as `DIAG` and a
 * secret `DASH_KEY` (the dashboard password).
 *
 * Abuse limits (the ingest URL ships inside a public app, so treat it as public):
 *  - body capped at 64 KB counted on the actual streamed bytes (chunked uploads
 *    and lying Content-Length headers can't get past it);
 *  - per-network limits keyed on a full IPv4 address or an IPv6 /64 (rotating
 *    addresses inside one /64 doesn't help): uploads per minute (optional
 *    `RL_IP` rate-limit binding, else per-isolate memory) and NEW device ids per
 *    hour (per-isolate memory);
 *  - KV writes are bounded per device: a device record is rewritten at most once
 *    per DEVICE_MIN_INTERVAL_MIN (default 120); the small `index` key (read by
 *    the dashboard instead of `list()`, whose calls share the 1,000/day quota)
 *    is only rewritten when a device joins, or its index timestamp is a day old;
 *  - the index holds at most MAX_DEVICES (default 60) entries. Entries silent for
 *    longer than the record TTL are pruned on every index write, and when it's
 *    full a NEW device evicts the least-established entry (fewest uploads, then
 *    least recently seen) instead of being refused — so a burst of fake ids can
 *    no longer lock real devices out. An evicted device rejoins on its next
 *    upload (its record keeps its own TTL).
 *  - The index update is read-merge-write, not atomic (KV has no CAS). Two
 *    concurrent joins can still drop one entry; the dropped device is re-added
 *    on its next upload (every upload checks it's listed), so the loss is at
 *    most one upload interval of dashboard visibility, never corruption. Index
 *    entries are validated on read, so a malformed index is repaired not fatal.
 *  - attacker-controlled keys (issue messages, perf/totals fields) are kept in
 *    null-prototype objects and `__proto__`/`constructor`/`prototype` are
 *    dropped, so they can't pollute prototypes.
 */

const MAX_ISSUES = 250; // cap stored distinct issues per device
const TTL_SECONDS = 45 * 24 * 3600; // forget a silent device after 45 days
const MAX_BODY = 65536;
const INDEX_KEY = 'index';
const IP_PER_MINUTE = 10; // in-memory fallback when RL_IP isn't bound
const NEW_DEVICES_PER_NET_HOUR = 5; // new device ids one IPv4 / IPv6 /64 may add per hour
const INDEX_REFRESH_MS = 24 * 3600 * 1000; // rewrite a device's index timestamp at most daily
const BAD_KEYS = new Set(['__proto__', 'constructor', 'prototype']);

export default {
  async fetch(request, env) {
    const url = new URL(request.url);

    if (request.method === 'POST' && url.pathname === '/ingest') {
      return ingest(request, env, url);
    }
    if (request.method === 'GET' && (url.pathname === '/diag' || url.pathname === '/')) {
      return dashboard(url, env);
    }
    return new Response('DropBeam diagnostics. POST /ingest · GET /diag?key=…', {
      status: 200,
      headers: { 'content-type': 'text/plain' },
    });
  },
};

async function ingest(request, env, url) {
  // Optional shared token: if you set an INGEST_TOKEN secret, the app's endpoint URL
  // must include ?t=THAT_TOKEN. Stops random internet clients (the URL is in a public
  // repo) from spamming your KV. Leave INGEST_TOKEN unset to accept any POST.
  if (env.INGEST_TOKEN && url.searchParams.get('t') !== env.INGEST_TOKEN) {
    return json({ ok: false, error: 'unauthorized' }, 401);
  }
  const net = ipKey(request.headers.get('cf-connecting-ip'));
  if (!(await allowIp(env, net))) return json({ ok: false, error: 'rate' }, 429);
  // Body-size cap (digests are tiny) so a hostile client can't inflate storage.
  const raw = await readCapped(request, MAX_BODY);
  if (raw === null) return json({ ok: false, error: 'too large' }, 413);

  let digest;
  try {
    digest = JSON.parse(raw);
  } catch {
    return json({ ok: false, error: 'bad json' }, 400);
  }
  const h = digest && typeof digest === 'object' ? digest.header : null;
  const deviceId = h && typeof h === 'object' && typeof h.deviceId === 'string' ? h.deviceId.slice(0, 64) : null;
  if (!deviceId) return json({ ok: false, error: 'no deviceId' }, 400);

  const key = `dev:${deviceId}`;
  const stored = await env.DIAG.get(key, 'json');
  const prevRec = stored && typeof stored === 'object' ? stored : null;
  // One write per device per interval, whatever the caller sends. Checked
  // before touching the index, so "too soon" calls cost no writes at all.
  if (prevRec && prevRec.lastSeen && Date.now() - prevRec.lastSeen < minIntervalMs(env)) {
    return json({ ok: false, error: 'too soon' }, 429);
  }

  const index = await readIndex(env);
  const mine = index.find((e) => e.k === key);
  if (!mine) {
    // A brand-new id (or one that was evicted / lost in an index race). Only
    // ids we have no record for count against the per-network join budget, so
    // a real device that fell out of the index always gets back in.
    if (!prevRec && !allowNewDevice(net)) return json({ ok: false, error: 'rate' }, 429);
    await writeIndex(env, { k: key, t: Date.now(), n: 1 });
  } else if (Date.now() - mine.t > INDEX_REFRESH_MS) {
    await writeIndex(env, { k: key, t: Date.now(), n: mine.n + 1 });
  }

  const prev = prevRec || {};
  const str = (v, max) => (typeof v === 'string' ? v.slice(0, max) : '');
  // Latest device metadata wins.
  prev.name = str(h.name, 80) || str(prev.name, 80);
  prev.appVersion = str(h.appVersion, 40) || str(prev.appVersion, 40);
  prev.os = `${str(h.os, 40)} ${str(h.arch, 20)}`.trim();
  prev.lastSeen = Date.now();
  prev.perf = flatRecord(digest.perf) || flatRecord(prev.perf);
  prev.totals = flatRecord(digest.totals) || flatRecord(prev.totals);

  // Merge issues by their message signature, accumulating counts. Keyed by
  // attacker-chosen text, so: a Map, and the prototype-ish keys are skipped.
  const issues = new Map();
  if (prev.issues && typeof prev.issues === 'object') {
    for (const [sig, it] of Object.entries(prev.issues)) {
      if (BAD_KEYS.has(sig) || !it || typeof it !== 'object') continue;
      issues.set(sig, { level: String(it.level || ''), msg: sig, count: Number(it.count) || 0, last: String(it.last || '') });
    }
  }
  for (const it of Array.isArray(digest.issues) ? digest.issues : []) {
    if (!it || typeof it !== 'object') continue;
    const sig = (typeof it.msg === 'string' ? it.msg : '').slice(0, 200);
    if (!sig || BAD_KEYS.has(sig)) continue;
    const cur = issues.get(sig) || { level: '', msg: sig, count: 0, last: '' };
    cur.count += Number(it.count) || 1;
    cur.last = String(it.last || cur.last || '').slice(0, 40);
    cur.level = String(it.level || cur.level || '').slice(0, 10);
    issues.set(sig, cur);
  }
  // Cap: keep the most recent / most frequent.
  const trimmed = [...issues.values()]
    .sort((a, b) => (a.last < b.last ? 1 : a.last > b.last ? -1 : b.count - a.count))
    .slice(0, MAX_ISSUES);
  const out = Object.create(null);
  for (const i of trimmed) out[i.msg] = i;
  prev.issues = out;

  await env.DIAG.put(key, JSON.stringify(prev), { expirationTtl: TTL_SECONDS });
  return json({ ok: true });
}

/** A shallow copy of a plain object with only primitive values, at most 40
 * keys, prototype-ish keys dropped (perf/totals are attacker-supplied). */
function flatRecord(v) {
  if (!v || typeof v !== 'object' || Array.isArray(v)) return null;
  const out = Object.create(null);
  let n = 0;
  for (const [k, x] of Object.entries(v)) {
    if (BAD_KEYS.has(k) || k.length > 40) continue;
    if (typeof x === 'number' || typeof x === 'boolean') out[k] = x;
    else if (typeof x === 'string') out[k] = x.slice(0, 80);
    else continue;
    if (++n >= 40) break;
  }
  return out;
}

async function dashboard(url, env) {
  if (url.searchParams.get('key') !== env.DASH_KEY) {
    return new Response('Forbidden — append ?key=YOUR_DASH_KEY', { status: 403 });
  }
  const devices = [];
  for (const { k } of await readIndex(env)) {
    const d = await env.DIAG.get(k, 'json');
    if (d) devices.push(d);
  }
  devices.sort((a, b) => (b.lastSeen || 0) - (a.lastSeen || 0));

  const esc = (s) =>
    String(s == null ? '' : s).replace(/[&<>"]/g, (c) => ({ '&': '&amp;', '<': '&lt;', '>': '&gt;', '"': '&quot;' }[c]));
  // Numeric fields come from untrusted /ingest JSON — coerce so a string like
  // "<img onerror=…>" can never render as raw HTML (stored-XSS guard).
  const num = (x) => Number(x) || 0;
  const ago = (ts) => {
    if (!ts) return '—';
    const m = Math.round((Date.now() - ts) / 60000);
    if (m < 60) return `${m}m ago`;
    if (m < 1440) return `${Math.round(m / 60)}h ago`;
    return `${Math.round(m / 1440)}d ago`;
  };
  const lvlColor = (l) => (l === 'error' ? '#ef4444' : l === 'warn' ? '#f59e0b' : '#64748b');

  const cards = devices
    .map((d) => {
      const issues = Object.values(d.issues || {}).sort((a, b) =>
        a.level === b.level ? b.count - a.count : a.level === 'error' ? -1 : 1,
      );
      const rows = issues
        .map(
          (i) => `<tr>
            <td><span style="color:${lvlColor(i.level)};font-weight:700">${esc(i.level)}</span></td>
            <td style="font-family:ui-monospace,monospace;font-size:12px">${esc(i.msg)}</td>
            <td style="text-align:right">${num(i.count)}</td>
            <td style="white-space:nowrap;color:#64748b">${esc(i.last)}</td>
          </tr>`,
        )
        .join('');
      const p = d.perf || {};
      return `<div class="card">
        <div class="head">
          <b>${esc(d.name) || '(unnamed)'}</b>
          <span class="meta">v${esc(d.appVersion)} · ${esc(d.os)} · seen ${ago(d.lastSeen)}</span>
        </div>
        <div class="perf">
          send ${num(p.sendAvgMBps)} MB/s · recv ${num(p.recvAvgMBps)} MB/s ·
          direct ${num(p.directPaths)} / relay ${num(p.relayPaths)} paths ·
          ${num(d.totals && d.totals.errors)} error-types
        </div>
        <table>${rows || '<tr><td colspan=4 style="color:#16a34a">No issues 🎉</td></tr>'}</table>
      </div>`;
    })
    .join('');

  const html = `<!doctype html><html><head><meta charset=utf-8>
  <meta name=viewport content="width=device-width,initial-scale=1">
  <title>DropBeam diagnostics</title>
  <style>
    body{font:14px system-ui,sans-serif;margin:0;background:#f8fafc;color:#0f172a}
    header{padding:18px 22px;background:#fff;border-bottom:1px solid #e2e8f0}
    h1{font-size:18px;margin:0}
    .wrap{padding:18px 22px;display:flex;flex-direction:column;gap:16px;max-width:1000px;margin:0 auto}
    .card{background:#fff;border:1px solid #e2e8f0;border-radius:12px;padding:14px 16px}
    .head{display:flex;justify-content:space-between;align-items:baseline;gap:10px}
    .meta{color:#64748b;font-size:12.5px}
    .perf{color:#475569;font-size:12.5px;margin:6px 0 10px}
    table{width:100%;border-collapse:collapse}
    td{padding:4px 6px;border-top:1px solid #f1f5f9;vertical-align:top}
    .empty{color:#64748b;padding:40px;text-align:center}
  </style></head><body>
  <header><h1>DropBeam — background diagnostics</h1>
    <div class="meta">${devices.length} device(s) · refreshed ${new Date().toUTCString()}</div></header>
  <div class="wrap">${cards || '<div class="empty">No diagnostics received yet.</div>'}</div>
  </body></html>`;
  return new Response(html, { headers: { 'content-type': 'text/html; charset=utf-8' } });
}

/** The body as text, or null past `max` bytes — counted while streaming. */
async function readCapped(request, max) {
  const declared = Number(request.headers.get('content-length') || 0);
  if (declared > max) return null;
  if (!request.body) return '';
  const reader = request.body.getReader();
  const chunks = [];
  let total = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) break;
    total += value.byteLength;
    if (total > max) {
      try { await reader.cancel(); } catch {}
      return null;
    }
    chunks.push(value);
  }
  const all = new Uint8Array(total);
  let at = 0;
  for (const c of chunks) { all.set(c, at); at += c.byteLength; }
  return new TextDecoder().decode(all);
}

/** Known devices as [{k:'dev:…', t:lastSeenMs, n:uploads}], validated so a
 * malformed or hand-edited index is repaired rather than fatal. Accepts the
 * older plain-string format. A pre-index deployment is migrated once from `list()`. */
async function readIndex(env) {
  const v = await env.DIAG.get(INDEX_KEY, 'json');
  if (Array.isArray(v)) return normalizeIndex(v);
  const list = await env.DIAG.list({ prefix: 'dev:' });
  const now = Date.now();
  const entries = list.keys.slice(0, maxDevices(env)).map((x) => ({ k: x.name, t: now, n: 1 }));
  await env.DIAG.put(INDEX_KEY, JSON.stringify(entries));
  return entries;
}

function normalizeIndex(v) {
  const seen = new Map();
  for (const e of Array.isArray(v) ? v : []) {
    // Old format: plain key strings. Treat as seen now so the upgrade doesn't
    // instantly prune every existing device.
    const ent = typeof e === 'string' ? { k: e, t: Date.now(), n: 1 } : e;
    if (!ent || typeof ent.k !== 'string' || !ent.k.startsWith('dev:') || ent.k.length > 68) continue;
    const t = Number(ent.t) || 0;
    const n = Math.max(1, Math.min(1e6, Number(ent.n) || 1));
    const cur = seen.get(ent.k);
    if (!cur) seen.set(ent.k, { k: ent.k, t, n });
    else { cur.t = Math.max(cur.t, t); cur.n = Math.max(cur.n, n); }
  }
  return [...seen.values()];
}

/** Upsert `entry` into the index. Re-reads right before writing and merges
 * (newest timestamp / highest count per key wins) to shrink the race window;
 * prunes entries silent past the record TTL; when still over the cap, evicts
 * the least-established entries (fewest uploads, then oldest) — never `entry`. */
async function writeIndex(env, entry) {
  const now = Date.now();
  const merged = normalizeIndex([...(await readIndex(env)), entry]);
  let live = merged.filter((e) => e.k === entry.k || now - e.t <= TTL_SECONDS * 1000);
  const cap = maxDevices(env);
  if (live.length > cap) {
    const others = live
      .filter((e) => e.k !== entry.k)
      .sort((a, b) => (b.n !== a.n ? b.n - a.n : b.t - a.t));
    live = [...others.slice(0, cap - 1), live.find((e) => e.k === entry.k)];
  }
  await env.DIAG.put(INDEX_KEY, JSON.stringify(live));
  return live;
}

function maxDevices(env) {
  return Math.max(1, Number(env.MAX_DEVICES) || 60);
}

function minIntervalMs(env) {
  const m = Number(env.DEVICE_MIN_INTERVAL_MIN);
  return (Number.isFinite(m) && m >= 0 ? m : 120) * 60 * 1000;
}

const ipCounts = new Map();
async function allowIp(env, net) {
  if (env.RL_IP && typeof env.RL_IP.limit === 'function') {
    try {
      return (await env.RL_IP.limit({ key: `ip:${net}` })).success;
    } catch {}
  }
  return bump(`m:${net}:${Math.floor(Date.now() / 60000)}`, IP_PER_MINUTE);
}

/** Per-network budget for registering new device ids (per isolate). */
function allowNewDevice(net) {
  return bump(`n:${net}:${Math.floor(Date.now() / 3600000)}`, NEW_DEVICES_PER_NET_HOUR);
}

function bump(bucket, max) {
  if (ipCounts.size > 50000) ipCounts.clear();
  const n = ipCounts.get(bucket) || 0;
  if (n >= max) return false;
  ipCounts.set(bucket, n + 1);
  return true;
}

/** Rate-limit key for a client address: a full IPv4 address (IPv4-mapped IPv6
 * counts as IPv4), or the first 4 hextets (/64) of an IPv6 address, expanded and
 * lowercased so every spelling of one /64 shares a bucket. Unparseable input
 * collapses to one shared 'unknown' bucket (Cloudflare always sends the header).
 * Same function as push-worker/worker.js — keep them in sync. */
function ipKey(raw) {
  let s = String(raw || '').trim().toLowerCase();
  if (!s) return 'unknown';
  if (s.startsWith('[') && s.endsWith(']')) s = s.slice(1, -1);
  const pct = s.indexOf('%'); // zone id
  if (pct >= 0) s = s.slice(0, pct);
  if (/^\d{1,3}(\.\d{1,3}){3}$/.test(s)) return validV4(s) ? `4:${s}` : 'unknown';
  if (!s.includes(':') || !/^[0-9a-f:.]+$/.test(s)) return 'unknown';
  // Trailing dotted IPv4 (e.g. ::ffff:1.2.3.4) → two hextets.
  const v4tail = s.match(/^(.*:)(\d{1,3}(?:\.\d{1,3}){3})$/);
  if (v4tail) {
    if (!validV4(v4tail[2])) return 'unknown';
    const o = v4tail[2].split('.').map(Number);
    s = v4tail[1] + ((o[0] << 8) | o[1]).toString(16) + ':' + ((o[2] << 8) | o[3]).toString(16);
  }
  const halves = s.split('::');
  if (halves.length > 2) return 'unknown';
  const part = (x) => (x === '' ? [] : x.split(':'));
  const head = part(halves[0]);
  const tail = halves.length === 2 ? part(halves[1]) : [];
  let groups;
  if (halves.length === 2) {
    const fill = 8 - head.length - tail.length;
    if (fill < 1) return 'unknown';
    groups = [...head, ...Array(fill).fill('0'), ...tail];
  } else {
    groups = head;
  }
  if (groups.length !== 8 || groups.some((g) => !/^[0-9a-f]{1,4}$/.test(g))) return 'unknown';
  const hx = groups.map((g) => parseInt(g, 16));
  // IPv4-mapped (::ffff:a.b.c.d) → the IPv4 address itself.
  if (hx.slice(0, 5).every((x) => x === 0) && hx[5] === 0xffff) {
    return `4:${hx[6] >> 8}.${hx[6] & 255}.${hx[7] >> 8}.${hx[7] & 255}`;
  }
  return `6:${hx.slice(0, 4).map((x) => x.toString(16)).join(':')}::/64`;
}

function validV4(s) {
  return s.split('.').every((o) => Number(o) <= 255);
}

function json(obj, status = 200) {
  return new Response(JSON.stringify(obj), {
    status,
    headers: { 'content-type': 'application/json', 'access-control-allow-origin': '*' },
  });
}
