# DropBeam background diagnostics — setup (5 minutes)

DropBeam can quietly upload a **redacted** error/performance digest from each
install (~once a day) so you can see the background problems users never report —
stalls, relay fallbacks, reconcile loops, slow speeds, errors. It never sends file
names or contents (see "What's collected" below).

The data goes to a tiny **Cloudflare Worker you own** (the same account your
feedback widget runs on). The app only ever uploads to the URL **you** put in
Settings — there is no baked-in endpoint, so this can't leak anywhere you didn't set.

You review everything on a single dashboard page.

---

## 1. Create the Worker

Cloudflare dashboard → **Workers & Pages → Create → Create Worker**.

- Name it `dropbeam-diag` (any name works — it just sets the URL).
- Click **Deploy**, then **Edit code**, paste the contents of `worker.js`
  (in this folder), and **Deploy** again.

Your endpoint is now: `https://dropbeam-diag.<your-subdomain>.workers.dev`
(yours is `https://dropbeam-diag.ashton-mcp-worker.workers.dev`).

> Prefer the CLI? `npm i -g wrangler`, then in this folder:
> `wrangler deploy worker.js --name dropbeam-diag`

## 2. Add storage (KV)

Workers & Pages → **KV** → **Create namespace**, name it `dropbeam-diag`.
Then open the Worker → **Settings → Variables → KV Namespace Bindings** → add:

- Variable name: **`DIAG`**  → your `dropbeam-diag` namespace. **Save.**

## 3. Set the dashboard password (and, recommended, an ingest token)

Worker → **Settings → Variables → Environment Variables** → add **Secrets**:

- **`DASH_KEY`** — any long random string (your dashboard password). **Required.**
- **`INGEST_TOKEN`** — another long random string. **Recommended:** since `worker.js`
  (with the example URL) lives in a public repo, this stops strangers POSTing junk.
  If set, the app's endpoint URL must end with `?t=THAT_TOKEN` (step 4). Leave it
  unset to accept any POST.

**Save & deploy.**

## 4. Point DropBeam at it

In DropBeam → **Settings → Share background diagnostics** (on by default):

- Set **Diagnostics endpoint** to: `https://dropbeam-diag.<your-subdomain>.workers.dev/ingest`
- Click **Send test** — you should see "Sent a test digest…". 
- Do the same on each of your machines (and your friend's). That's the only per-device step.

## 5. Review daily

Open: `https://dropbeam-diag.<your-subdomain>.workers.dev/diag?key=YOUR_DASH_KEY`

One card per device — name, version, OS, last-seen, average send/recv speeds,
direct-vs-relay path counts, and a table of distinct issues with how often each
happened and when it last occurred. Errors are sorted to the top.

---

## Abuse limits (built in)

The ingest URL ships inside the app, so the Worker treats it as public:

- Bodies over 64 KB are refused, counted on the bytes actually received (a
  chunked upload or a wrong `Content-Length` can't get past it).
- Each network gets 10 uploads a minute, and may register at most 5 new
  device ids an hour. A "network" is one IPv4 address or one IPv6 /64, so
  rotating addresses inside a /64 doesn't get around it. For an upload limit
  shared across the isolates in a Cloudflare location, add a rate-limit binding
  to the Worker (a `wrangler.toml` next to `worker.js`):

  ```toml
  [[ratelimits]]
  name = "RL_IP"
  namespace_id = "7201"
  simple = { limit = 10, period = 60 }
  ```

- KV writes stay small. Each device's record is rewritten at most once every
  2 hours. The dashboard reads one `index` key instead of `list()`. That key is
  only rewritten when a device joins, or about once a day per device to refresh
  its last-seen time. Tune with the plain variables `DEVICE_MIN_INTERVAL_MIN`
  and `MAX_DEVICES`. A device that uploads too soon gets HTTP 429 (no writes);
  the app keeps those log lines and sends them with its next digest, so nothing
  is lost. **Send test** twice within 2 hours reports that 429. That's expected.
- The index never locks real devices out. Entries silent for 45 days are pruned
  on every index write. When it's full (60 by default), a new device evicts the
  least-established entry (fewest days seen, then least recently seen). Junk
  ids from a flood evict each other before they evict devices that report
  daily, and an evicted device rejoins on its next upload.
- Known limits: KV has no atomic update, so two devices joining at the same
  instant can drop one index entry. That device reappears on its next upload.
  Someone with many separate IPv4 addresses or /64s can still cause extra KV
  writes. Setting `INGEST_TOKEN` (step 3) shuts that out.
- Message text and other keys from the app are stored in prototype-free
  objects; `__proto__`, `constructor` and `prototype` keys are dropped.

To remove a device for good, delete its `dev:…` key **and** its entry in the
`index` key (KV → `dropbeam-diag`). The index format is a list of
`{"k":"dev:…","t":lastSeenMs,"n":daysSeen}`; the old plain list of key strings is
still read and upgraded automatically.

## What's collected (and what isn't)

**Sent:** a random per-install id, your chosen display name, app version, OS/arch,
counts of errors/warnings, average transfer speeds, direct-vs-relay path counts, and
de-duplicated **error/warning message text** with file paths, file names, IP
addresses, and long ids stripped out.

**Never sent:** file names, file contents, folder contents, secrets, or full paths.

Anyone can turn it off in Settings (the toggle), and with no endpoint URL set it
uploads nowhere at all.

## Optional: mirror into a private GitHub repo

If you'd rather browse logs in GitHub, the Worker can also commit each device's
record to a private repo — add a `GH_TOKEN` (a fine-grained PAT with Contents:write
on that repo) + `GH_REPO` (`owner/name`) and a small commit step in `ingest()`. The
KV dashboard already gives you everything, so this is purely optional. Ask and I'll
wire it.
