// DropBeam short-code rendezvous — a tiny Cloudflare Worker.
//
// It swaps a short word-code for an iroh "ticket" (a few bytes) so a one-off
// Quick Send can use a short code instead of a long link/QR. It NEVER sees your
// files — those flow pure peer-to-peer over iroh, end-to-end encrypted. The
// worker only stores { code -> ticket } for a few minutes, then it expires.
//
// Deploy: see SHORT-CODES.md. Requires a KV namespace bound as DROPBEAM_KV.

const CORS = {
  'Access-Control-Allow-Origin': '*',
  'Access-Control-Allow-Methods': 'GET, POST, OPTIONS',
  'Access-Control-Allow-Headers': 'Content-Type',
}

export default {
  async fetch(request, env) {
    if (request.method === 'OPTIONS') {
      return new Response(null, { headers: CORS })
    }
    const url = new URL(request.url)

    // Sender registers a fresh code -> ticket mapping.
    if (request.method === 'POST') {
      let body
      try {
        body = await request.json()
      } catch {
        return json({ error: 'bad json' }, 400)
      }
      const code = String(body.code || '').trim().toLowerCase()
      const ticket = String(body.ticket || '').trim()
      if (!code || !ticket) return json({ error: 'code and ticket required' }, 400)
      // Short TTL so a code can't be reused or harvested later.
      await env.DROPBEAM_KV.put('c:' + code, ticket, { expirationTtl: 600 })
      return json({ ok: true })
    }

    // Receiver looks up the ticket for a code.
    if (request.method === 'GET') {
      const code = String(url.searchParams.get('code') || '').trim().toLowerCase()
      if (!code) return json({ error: 'code required' }, 400)
      const ticket = await env.DROPBEAM_KV.get('c:' + code)
      if (!ticket) return json({ error: 'not found or expired' }, 404)
      return new Response(ticket, { headers: { ...CORS, 'Content-Type': 'text/plain' } })
    }

    return json({ error: 'method not allowed' }, 405)
  },
}

function json(obj, status = 200) {
  return new Response(JSON.stringify(obj), {
    status,
    headers: { ...CORS, 'Content-Type': 'application/json' },
  })
}
