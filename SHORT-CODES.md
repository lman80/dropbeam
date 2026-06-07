# Short word-codes for Quick Send

DropBeam now moves **every** transfer directly peer-to-peer over **iroh** — no
croc, no relay servers carrying your files. For friends and Shared Drop Folders
that's automatic (send by name). For a **one-off Quick Send to someone who isn't
a saved friend**, the receiver needs a way to find the sender. Two options:

1. **Link / QR (works today, nothing to set up).** The sender shows a Direct
   link/QR; the receiver pastes it (or scans). Files flow pure P2P over iroh.

2. **Short word-codes (optional).** Like the old `apple-banana-cat` codes you
   read aloud. A short code can't contain a full network address, so it needs a
   tiny **rendezvous broker** that swaps `code → ticket`. The broker only brokers
   a few bytes — **it never sees your files**, which still go end-to-end
   encrypted P2P over iroh.

This repo ships that broker (`broker/dropbeam-rendezvous.js`) as a free
Cloudflare Worker. It's not wired into the app yet — once you deploy it and give
me the URL, I'll switch short codes back on and test against your live broker.

## Deploy the broker (~5 minutes, free)

1. Create a free Cloudflare account at https://dash.cloudflare.com/sign-up.
2. In the dashboard: **Workers & Pages → Create application → Create Worker**.
   Name it e.g. `dropbeam-rendezvous`, click **Deploy** (the placeholder code is
   fine for now).
3. **Storage & Databases → KV → Create a namespace**, name it `DROPBEAM_KV`.
4. Open your Worker → **Settings → Variables → KV Namespace Bindings → Add
   binding**. Variable name: `DROPBEAM_KV`; KV namespace: the one you just made.
   Save.
5. Worker → **Edit code**, paste the contents of
   `broker/dropbeam-rendezvous.js`, **Save and deploy**.
6. Copy your Worker URL (looks like
   `https://dropbeam-rendezvous.<you>.workers.dev`).

Send me that URL and I'll enable short codes in the app.

## Notes

- **Privacy/security:** the broker stores `code → ticket` for ~10 minutes max,
  then it expires. The ticket is single-use (consumed on first pull). This is the
  same trust model the old croc code had, minus croc relaying your file bytes.
- **Self-hosted & swappable:** it's your Worker on your account. You can move it,
  delete it, or point the app at a different broker anytime.
