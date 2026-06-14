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
 */

const MAX_ISSUES = 250; // cap stored distinct issues per device
const TTL_SECONDS = 45 * 24 * 3600; // forget a silent device after 45 days

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
  // Body-size cap (digests are tiny) so a hostile client can't inflate storage.
  const len = Number(request.headers.get('content-length') || 0);
  if (len > 65536) return json({ ok: false, error: 'too large' }, 413);

  let digest;
  try {
    digest = await request.json();
  } catch {
    return json({ ok: false, error: 'bad json' }, 400);
  }
  const h = digest && digest.header;
  const deviceId = h && typeof h.deviceId === 'string' ? h.deviceId.slice(0, 64) : null;
  if (!deviceId) return json({ ok: false, error: 'no deviceId' }, 400);

  const key = `dev:${deviceId}`;
  const prev = (await env.DIAG.get(key, 'json')) || { issues: {} };

  // Latest device metadata wins.
  prev.name = h.name || prev.name || '';
  prev.appVersion = h.appVersion || prev.appVersion || '';
  prev.os = `${h.os || ''} ${h.arch || ''}`.trim();
  prev.lastSeen = Date.now();
  prev.perf = digest.perf || prev.perf || null;
  prev.totals = digest.totals || prev.totals || null;

  // Merge issues by their message signature, accumulating counts.
  const issues = prev.issues || {};
  for (const it of digest.issues || []) {
    const sig = (it.msg || '').slice(0, 200);
    if (!sig) continue;
    const cur = issues[sig] || { level: String(it.level || ''), msg: sig, count: 0, last: '' };
    cur.count += Number(it.count) || 1;
    cur.last = String(it.last || cur.last || '');
    cur.level = it.level || cur.level;
    issues[sig] = cur;
  }
  // Cap: keep the most recent / most frequent.
  const trimmed = Object.values(issues)
    .sort((a, b) => (a.last < b.last ? 1 : a.last > b.last ? -1 : b.count - a.count))
    .slice(0, MAX_ISSUES);
  prev.issues = Object.fromEntries(trimmed.map((i) => [i.msg, i]));

  await env.DIAG.put(key, JSON.stringify(prev), { expirationTtl: TTL_SECONDS });
  return json({ ok: true });
}

async function dashboard(url, env) {
  if (url.searchParams.get('key') !== env.DASH_KEY) {
    return new Response('Forbidden — append ?key=YOUR_DASH_KEY', { status: 403 });
  }
  const list = await env.DIAG.list({ prefix: 'dev:' });
  const devices = [];
  for (const k of list.keys) {
    const d = await env.DIAG.get(k.name, 'json');
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

function json(obj, status = 200) {
  return new Response(JSON.stringify(obj), {
    status,
    headers: { 'content-type': 'application/json', 'access-control-allow-origin': '*' },
  });
}
