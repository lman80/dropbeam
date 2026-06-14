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
