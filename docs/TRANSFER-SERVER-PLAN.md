# Transfer Server — design plan (GitHub #45)

Status: design only, nothing implemented. Baseline: branch `ios` @ `a618186` (v0.52.x, iroh 1.2 vendored).
Scope: store-and-forward of **chat + friend file sends** through a DropBeam device the user chooses, plus iOS push. Folder sync, Locations and Quick Send are out of scope for v1.

---

## 1. Summary (plain language)

Today DropBeam only delivers when both people are online at the same time. A **Transfer Server** is one of your own always-on devices (for the owner: the Linux box) that you switch into "hold things for people" mode. When you send a message or file to someone who is offline, DropBeam notices within a few seconds, **locks the content so only the recipient can open it**, and hands it to the Transfer Server. The moment the recipient comes online, the server delivers it and deletes its copy; your chat shows "Held on Linux Box" and then "Delivered". You can let friends use your server too — for messages *to* you, and optionally for their messages to anyone. iPhones get a notification ("New message from Ashton") through a tiny push relay the owner runs on Cloudflare, so someone whose phone is in their pocket still finds out. The server can never read what it carries, and it cleans up after itself automatically (limits, expiry, delete-after-delivery).

---

## 2. User stories

1. **Owner, offline friend.** I send Alex a photo at 11pm; Alex's laptop is asleep. My Mac says "Alex is offline — held on Linux Box (will reach Alex when they're online)". I close my Mac. At 8am Alex opens his laptop and the photo is there; my chat shows "Delivered".
2. **Friend → offline owner.** Alex messages me while all my devices are off. Because I let my friends leave things on my Linux box, his app deposits there. When my phone next opens, everything is waiting.
3. **iPhone in a pocket.** Alex has an iPhone and DropBeam is suspended. My message lands on the server; Alex's lock screen shows "Ashton: running 10 min late". Tapping opens DropBeam, which fetches the message.
4. **Shared server.** My friend Mong has no server. I invite her to use mine; now when she sends to Alex while Alex is offline, it's held on my box (encrypted — I can't read it, only see sizes/counts).
5. **Setup in under a minute.** On the Linux box I open Settings → Transfer Server → "Make this a Transfer Server", pick "Store up to 200 GB on /mnt/buddy", pick "Me + my friends", done. My other devices pick it up automatically.
6. **Control.** I can see who is using my server, how much space each person is using, remove a person (their held items are deleted, they're notified "no longer available"), pause the server, or wipe it.
7. **Nothing lost silently.** If an item expires (recipient gone 30 days) or the server is full, the sender's chat shows exactly that with a one-tap "Try again directly".

---

## 3. What already exists that this builds on (read in code)

| Piece | Where | Reuse |
|---|---|---|
| Identity = iroh ed25519 seed, `EndpointId` | `iroh_net.rs::load_or_create_secret`, `write_private` | server/recipient identity, signatures (`SecretKey::sign`, as in `link.rs:132`/`account.rs:1990`) |
| One ALPN, framed JSON streams dispatched by `kind` | `iroh_net.rs::serve_stream_inner` (`Some("ping")`, `"chat"`, `"files"`, `"locations.*"`, `"account-sync"`…) | add `"mailbox.*"` kinds the same way `locations.*` is namespaced |
| Blocked-peer gate before handlers | `is_blockable_kind` / `serve_blocked` | include `mailbox.deposit` |
| Capability negotiation in hello | `friend-hello` reply carries `progress_v`, `locations_v`; `learn_progress` | add `mailbox_v` + `servers` advertisement |
| Durable chat outbox + op-outbox, per-peer backoff 12s→60s→300s | `spawn_chat_outbox_retry`, `chat::outbox`, `chat::pending_ops`, `send_chat_any` | the "deposit on offline" hook; the server's delivery loop copies this loop's shape |
| Chat idempotency | `chat::append` dedups by `(id, from_me)` and by `file_xfer_id` | direct + server double delivery is harmless |
| Status ladder | `ChatMessage.status` "sending/delivered/read/failed", `chat::status_rank` | add **"held"** |
| Presence | `handle_conn` → `emit_friend_presence` (45s RX tick), `account::device_seen`, `src/lib/presence.ts` (120s window), `ping_endpoint` (15s), `send_chat` 12s dial, `FRIEND_SEND_RETRY_SECS = 90` | fast offline detection |
| Resumable ranged file transfer + sha256 end-to-end | `send_ranges_hashed`, `recv_file_resumable_hashed`, `Coverage`, `PartialSidecar`, `PARTIAL_TTL_SECS`, `integrity.rs`, `verify.rs` | resumable uploads to / downloads from the server |
| Safe storage root on a NAS | `locations::Root` (`O_NOFOLLOW`, same-device, `marker`, `MountChanged`, `UnsafeRoot`), `default_byte_cap`, `LocationError::{Quota,Busy,RateLimit}` | server's storage directory + quota/rate errors |
| Multi-device people | `friends::person_endpoints`, `friends::thread_owner`, account sync in `account.rs` | seal to every device of the recipient; replicate "my servers" to own devices |
| Custom relay | `settings.custom_relay` → `RelayMode::Custom` in `iroh_net::start`; `RELAY-SETUP.md` | phase 3 relay option |
| Crash-safe JSON stores | `settings::write_atomic`, `write_atomic_with_backup`, `read_json_store` | server index |
| iOS notifications | vendored `tauri-plugin-notification` (local only), `NotificationHandler.swift`; entitlements file is **empty** (no `aps-environment`) | phase 2 |
| iOS project | XcodeGen `src-tauri/gen/apple/project.yml` (team `R2RDA8476R`, bundle `com.ashtonmiller.dropbeam`) | add NSE target |
| Cloudflare worker pattern | diagnostics worker / `broker/dropbeam-rendezvous.js` (`SHORT-CODES.md`) | push relay worker |

Gaps found: no headless/daemon mode (`main.rs`/`lib.rs` have none — the Linux box runs the GUI `.deb` in a logged-in GNOME session); no background modes/APNs on iOS; `chat.rs` header still says "no store-and-forward yet".

---

## 4. UX flows

Design rule: one noun everywhere — **Transfer Server** (not "mailbox", "relay", "inbox"). Status copy always names the device and the person.

### 4.1 Animated guides (shared across desktop + iOS)

A 3-scene looping explainer (Lottie or pure CSS/SwiftUI; ~8s loop, respects Reduce Motion → static frames with captions):

1. **"When they're offline…"** — Your phone on the left, Alex's sleeping laptop (Zzz) on the right. A paper plane leaves your phone, bumps the sleeping laptop, bounces.
2. **"…it waits on your Transfer Server"** — the plane re-routes into a small server box in the middle; a padlock snaps shut on it ("Locked — only Alex can open it"). Your phone goes dark.
3. **"…and arrives when they're back."** — Alex's laptop wakes (sun), the box glows, the plane flies to Alex; a check mark appears on your phone: "Delivered". The box empties (trash-can puff: "Deleted from the server").

A second 2-scene clip for sharing: "Friends can use it too" — three friend avatars, arrows into your box, padlocks on every item, a gauge showing space used.

A third 2-scene clip for iPhone push: phone in pocket → buzz with a lock-screen banner → tap → message opens.

Shown: first-run of the Transfer Server settings page, the "What's this?" link on every "Held on…" status, and the first time a friend's app offers to use your server.

### 4.2 Owner setup wizard (desktop, the server device)

Settings → **Transfer Server** (new section, desktop only; hidden on iOS since iOS cannot host).

```
┌ Transfer Server ─────────────────────────────────────────┐
│ [ animated explainer, scene 1→3 ]                        │
│ Hold messages and files for people who are offline.      │
│ Everything stays locked — this device can't read it.     │
│                                                          │
│ This device is a good choice ✓ plugged in  ✓ always on   │
│   (warns: "This Mac sleeps — items wait until it wakes") │
│                                                          │
│            [ Make this a Transfer Server ]               │
└──────────────────────────────────────────────────────────┘
Step 2 — Where and how much
  Store in: [ /mnt/buddy/DropBeam Transfer ▾ ]  (Choose…)
  Limit:    [====|-----] 200 GB of 1.8 TB free
  Keep undelivered items for: ( 7 days ) (•14 days) ( 30 days )
Step 3 — Who can use it
  (•) Me and my devices only
  ( ) Me + friends I choose   [ ☑ Alex ☐ Mong … ]
  ( ) Me + all my friends
  [i] "Friends can leave things for you here, and send to
       anyone through it when you allow 'send through'."
  ☐ Also let them send to other people through this server
Step 4 — Done
  "Linux Box is now your Transfer Server."
  Your devices: Mac ✓ iPhone ✓ (auto-switched via account sync)
  [ Tell my friends ]  → sends an offer card in each chosen chat
```

Checks at step 1: platform (`device_os`), on battery/sleep settings (macOS `pmset`, Linux `systemd-inhibit` availability), free space. Storage folder validation reuses `locations::Root::for_location` (rejects `UnsafeRoot`, records mount `marker`/`device` so an unmounted NAS fails closed with `MountChanged` instead of filling the root disk).

### 4.3 Server management page (after setup)

```
Transfer Server — Linux Box                 ● Running   [Pause]
 Storage  ▓▓▓▓░░░░░░  38 GB of 200 GB · 12 items waiting
 People
   You (4 devices)            3 items · 1.2 GB
   Alex                       0 items
   Mong  (can send through)   9 items · 36.8 GB   [Remove]
 Waiting for delivery
   → Alex   2 items  1.1 GB  held 3h   expires in 13d
   → Bob    9 items  36 GB   held 2d   expires in 12d
 [Change limit]  [Change who can use it]  [Delete everything]
```

Names only for people the server knows; unknown recipients show as "someone Mong knows" + short id. No filenames (server doesn't have them).

### 4.4 Friend opt-in (the user of someone else's server)

When a friend's hello advertises a server that grants us access, a one-time card appears in that chat (and in Settings → Transfer Server → "Servers I can use"):

```
┌──────────────────────────────────────────────┐
│ [scene: friends → box]                       │
│ Ashton shared a Transfer Server with you.    │
│ Things you send to Ashton will wait there    │
│ when he's offline. Everything stays locked.  │
│ [ Use it ]   [ Not now ]                     │
│ ☐ Also hold messages for me there            │  ← registers push (iOS)
└──────────────────────────────────────────────┘
```

Inbox use (messages *to* the server owner) needs no opt-in from the sender; it's automatic once the owner advertises it. "Send through" (deposit for third parties) and "hold messages for me" (the friend's own inbox + push registration) are explicit.

### 4.5 Status surfaces

Chat bubble ladder (sender): `Sending…` → `Alex is offline · Held on Linux Box` (server glyph) → `Delivered` → `Read`. Failure variants: `Couldn't reach Linux Box — will keep trying`, `Linux Box is full — sending directly when Alex is online`, `Expired on Linux Box after 14 days · [Send again]`.

Transfer card (files): progress bar labeled "Uploading to Linux Box (locked)" then "Held on Linux Box — will reach Alex when they're online"; a later `mailbox.status` refresh turns it into "Delivered to Alex". If Alex appears mid-upload: "Alex came online — sending directly" (server upload cancelled).

Recipient: items delivered via a server look like normal messages; a tiny caption "via Linux Box · sent 8h ago" on the first item of a batch, timestamp = original send time.

iOS: same ladder in `ConversationView`; Settings gets "Servers I can use" and "Notifications from servers" rows; no hosting UI.

---

## 5. Architecture

### 5.1 Roles

- **Server (S)**: a desktop DropBeam with `transfer_server.enabled`. Accepts deposits, stores sealed blobs, delivers, deletes. Owner-controlled ACL.
- **Sender (A)**: any DropBeam ≥ `mailbox_v:1`. Knows a set of usable servers.
- **Recipient (B)**: one person = N devices (`person_endpoints`). Accepts `mailbox.deliver` from any server when the inner envelope is signed by one of B's friends; fetches from known servers on launch/foreground.
- **Push relay (P)**: owner's Cloudflare Worker; holds the APNs `.p8`; forwards opaque pushes (phase 2).

Which server does A choose? In order:
1. **Recipient's inbox servers** (B advertised "hold messages for me at S_B" in hello) — like email MX; B checks them itself and registers push there.
2. **A's own server** (A is owner or allowed "send through").
3. Neither → today's behavior (outbox retries directly).

Both A and B advertise in `friend-hello` (both request and reply, next to `progress_v`/`locations_v`):
```json
"mailbox_v": 1,
"mailbox_key": "<x25519 pub, b64>", "mailbox_key_sig": "<ed25519 sig by endpoint key>",
"servers": [{"eid":"<S endpoint>","name":"Linux Box","inbox":true,"through":false,"exp":<ms>}]
```
`servers` entries are only trusted because the hello arrives on an authenticated connection (`conn.remote_id()`), same trust as today's name/avatar. Own devices additionally replicate `transfer_servers` via `account.rs` sync (add to the synced settings allowlist).

### 5.2 Offline detection ("quickly")

There is no authoritative "offline" in P2P; the design makes a fast *guess* and makes wrong guesses harmless (dedupe).

Signals, cheapest first (new fn `iroh_net::reachability(eid) -> Hint`):
1. **Live**: a cached `friend_conns` entry whose `refresh_presence` saw new RX in the last 20s, or any accepted conn in the last 20s (`conn_recently_accepted`) → `Online`.
2. **Recently seen**: `device_seen` / frontend presence < 120s → `Probably online` → normal direct path with existing timeouts.
3. **Otherwise probe**: dial with a **4s budget** (`ep.connect` wrapped in `timeout`). LAN/direct connects in <1s and relay-mediated in 1–3s on healthy relays; an offline peer never answers, so 4s is the practical floor. iOS peers (`Friend.device_os == "ios"`) not seen in 120s are treated as offline immediately (they're suspended in background).
4. Tri-state result → **Offline** triggers deposit.

Chat: `send_chat_any` currently dials 12s per device; with a server available, the outbox's first attempt uses `reachability`; on `Offline` it deposits and marks `held`. Messages in `held` are excluded from `chat::outbox()` direct retries (server owns delivery) but a *direct* delivery still happens opportunistically if B connects to A first (B's hello → A wakes outbox → sends direct; both copies dedupe in `chat::append`).

Files: `send_friend_inner` keeps its 90s re-dial window only when no server is available. With a server: one 6s direct attempt, then "Uploading to Linux Box". A background `reachability` re-probe every 30s during the upload; if B turns reachable **and** less than 50% is uploaded, cancel the deposit (`mailbox.cancel`) and go direct; otherwise finish the deposit (server will deliver within seconds, since B is online).

### 5.3 How the recipient gets pending items

Two paths, both idempotent:

- **Push-from-server (primary for desktops).** S runs a delivery loop modeled on `spawn_chat_outbox_retry`: for each recipient endpoint with pending items, dial with backoff 15s → 60s → 5min → 15min cap; also woken instantly when that endpoint connects to S for any reason (`handle_conn` → `emit_friend_presence` hook → `wake_mailbox(eid)`). Delivery = `mailbox.deliver` stream.
- **Pull-by-recipient (primary for iOS, and after long absences).** On app start / foreground / push tap / network change, B sends `mailbox.fetch` to every known inbox server + every server advertised by its friends that is currently reachable (bounded: ≤8 servers, parallel, 5s each). S answers with a list of item ids for B's endpoint, then streams them.

A multi-device recipient: items are sealed to all of B's devices' mailbox keys (see 5.5); the first device to ack consumes it; the item is **deleted after the first device ack** and account sync (`account.rs` chat merge) spreads chat to B's other devices; files land on whichever device fetched (same as a direct send to a person today).

### 5.4 Protocol messages (new `kind`s on `dropbeam/1`)

All are JSON header frame (`write_frame`/`read_frame_cap`, ≤ `MAX_HEADER` 1 MiB) followed by raw bytes where noted. Every server-side handler first checks `conn.remote_id()` against the ACL.

| kind | direction | body | notes |
|---|---|---|---|
| `mailbox.hello` | A/B → S | `{v, want:["deposit","fetch","through"]}` → `{v, quota:{used,cap,item_max}, expiry_days, rights}` | capability + quota preview |
| `mailbox.deposit` | A → S | `{v, item_id, to:[eid…], kind:"chat"\|"op"\|"file", size, segs, seg_size, ct_sha256, expires, env_sig}` then ciphertext segments (resumable: reply `{have:[ranges]}`) | `item_id` = uuidv4 chosen by A; re-deposit same id resumes; returns `{ok, held_until}` |
| `mailbox.cancel` | A → S | `{item_id}` | only the depositor |
| `mailbox.status` | A → S | `{item_ids:[…]}` → `[{id, state:"held"\|"delivered"\|"expired"\|"rejected", at}]` | S keeps a 30-day receipt ledger (ids + state only) |
| `mailbox.fetch` | B → S | `{v, since?}` → `{items:[{item_id,size,from_hint,kind,ts}]}` then per-item `mailbox.get` | lists only items whose `to` contains `remote_id()` |
| `mailbox.get` | B → S | `{item_id, have:[ranges]}` → header + ciphertext | ranged, reuses `Coverage` semantics |
| `mailbox.deliver` | S → B | same payload as get, pushed | B replies `{ack:true}` only after durable store + decrypt + sha256 OK |
| `mailbox.ack` | B → S | `{item_id, ok, reason?}` | S deletes blob, records receipt |
| `mailbox.push-register` | B(iOS) → S | `{sealed_token, push_key_pub, exp}` | phase 2, see §6 |

Version gate: send only to peers that advertised `mailbox_v`; S answers unknown kinds from old peers via the existing `_ => ok` path in `serve_blocked` semantics.

### 5.5 Encryption (end-to-end, server-blind)

**Keys.** Each device creates a dedicated **X25519 mailbox key** (random, stored like the iroh seed: `mailbox-key.key`, 0600 via `write_private`; iOS: Keychain in a shared access group so the Notification Service Extension can read it). Its public half is published in hello with `mailbox_key_sig = ed25519_sign(endpoint_key, "dropbeam-mailbox-key-v1" || x25519_pub)`. Why not convert the ed25519 identity to X25519? It works mathematically (curve25519-dalek `to_montgomery`) but reusing a signing key for key agreement is poor hygiene, and a separate key can be rotated and shared with the iOS extension without exposing the iroh identity. Senders only seal to keys whose signature verifies against the recipient's pinned `endpoint_id`.

**Envelope (age/HPKE-style, multi-recipient).** Algorithms chosen so both Rust (`ring`, already a direct dep) and Swift CryptoKit (NSE) implement them without new crates:

```
file_key        = 32 random bytes
for each recipient device R (person_endpoints of B):
    eph         = X25519 ephemeral keypair            (per recipient)
    ss          = X25519(eph, R.mailbox_pub)
    wrap_key    = HKDF-SHA256(ikm=ss, salt=eph_pub||R.mailbox_pub, info="dropbeam-mbx-wrap-v1")
    stanza      = {eid: R, eph_pub, ct: ChaCha20-Poly1305(wrap_key, nonce=0, file_key)}
header          = {v:1, item_id, from: A.eid, to:[eids], kind, created_ms, stanzas[], meta_ct}
meta_ct         = AEAD(K_meta, {chat payload (chat_payload JSON) | file manifest names/sizes/mtimes/sha256, seq})
payload         = STREAM: 1 MiB segments, AEAD(K_payload, nonce = 11-byte BE counter || last_flag)
K_meta, K_payload = HKDF(file_key, info="…meta-v1" / "…payload-v1")
signature       = A.endpoint_key.sign("dropbeam-mbx-env-v1" || sha256(header_without_sig) || sha256(all ciphertext))
```

- Crates: **`ring` 0.17** (direct dep: `aead::CHACHA20_POLY1305`, `hkdf::HKDF_SHA256`, `rand::SystemRandom`). X25519 with a *static* recipient key is not exposed by ring (its `agreement` API is ephemeral-only), so recipient-side agreement uses **`curve25519-dalek` 5.0** (already in `Cargo.lock` via `iroh-base`; add as a direct dep at the same version → zero new downloads): `MontgomeryPoint::mul_clamped` / `mul_base_clamped`. Signatures use iroh's `SecretKey::sign` / `PublicKey::verify` (ed25519-dalek 3.0 underneath), exactly as `link.rs` does. SHA-256 via existing `sha2`/`ring`.
- Swift NSE: `Curve25519.KeyAgreement`, `HKDF<SHA256>`, `ChaChaPoly` — all CryptoKit, same byte layout. A shared test vector file (`docs/mailbox-vectors.json`) is generated in Rust tests and checked by an XCTest.
- What the server sees: sender eid (it authenticated A anyway), recipient eids, sizes, timing, `kind` (chat/op/file — could be hidden by padding; see decision 7). It never sees text, filenames, file contents.
- End-to-end integrity: the plaintext sha256 per file (existing `integrity.rs` digests) is inside `meta_ct`; the recipient verifies after decrypt, same as a direct send. The server verifies only `ct_sha256` (ciphertext) so it can reject corrupt uploads and resume.
- Authenticity: B checks the signature against A's pinned `endpoint_id` **and** that A is B's friend (`friends::chat_sender`), else drops (acks `ok:false, reason:"unknown sender"` so S deletes it). A malicious server can therefore drop, delay or replay, but not forge or read; replays are neutralized by `item_id` dedupe (`mailbox-seen.json`, 60-day bounded set) + `chat::append` dedupe.

### 5.6 Server storage format

Under the chosen root (default `<config>/transfer-server/`, or a validated NAS folder):
```
transfer-server/
  .dropbeam-server-marker          # mount identity (locations-style), fail closed if missing
  index.json                       # write_atomic_with_backup; small: items + per-user usage
  receipts.jsonl                   # item_id,state,at (30-day rolling, compacted daily)
  items/<first2 of item_id>/<item_id>/
      header.json                  # the sealed envelope header (as received, signature intact)
      payload.part + payload.cov   # during upload; cov = Coverage sidecar (reuse PartialSidecar)
      payload                      # after ct_sha256 verified (rename)
```
`index.json` item: `{item_id, from, to[], kind, size, created, expires, state, delivered_to?, bytes_on_disk}`. Recovered on startup by scanning `items/` if `index.json` is unreadable (`read_json_store` → rebuild). Deleting after delivery = remove the directory, then update index. No filenames of user content ever touch disk in clear.

**Quotas / limits (defaults, owner-editable):** total cap (wizard slider; default min(100 GB, 50% free)); free-space floor 5 GB or 5% (refuse new deposits below it); per-user cap 25% of total (owner exempt); per-item max 20 GB (configurable); items per recipient 2,000; expiry 14 days (chat items: 30 days — tiny); rate limits reuse `LocationError::RateLimit`-style token bucket (10 req/s, 2 concurrent uploads per user like `LocationError::Busy`).

**Dedupe.** Content dedupe across recipients would require convergent encryption (leaks equality) — rejected. Dedupe that *is* done: (a) one ciphertext for all of a person's devices and for a group send to several friends (multiple stanzas, one payload); (b) idempotent `item_id` re-deposit; (c) resume by ranges.

### 5.7 Delivery ordering, receipts, chat semantics

- Per (sender, recipient) S delivers in `created_ms` order; `ChatMessage.seq` (Lamport) + `order_key` still decide on-screen order, so a direct message that races past a held one sorts correctly.
- Edits/unsends/reactions (`ChatOp`) are deposited as `kind:"op"` items; the op-outbox gate (`message_status ∈ {delivered, read}`) is extended to accept `held` **only when the target was held on the same server** (FIFO guarantees order).
- Delivered receipts: A learns via (1) `mailbox.status` whenever A next talks to S (the outbox loop polls held items every 5 min while online, and on A's startup), (2) B's normal `chat-signal` read receipt later. S never needs to reach A.
- Typing indicators never go through servers. Read receipts (`chat-signal` read) may be deposited as tiny `op` items if A is offline (respecting `send_read_receipts`).
- File sends: B's receive path is the existing one after decryption — decrypted segments stream into the normal staging (`receive_stage.rs`) → `publish_unique` → chat file message with `file_xfer_id` = original transfer id, so dedupe with a late direct copy works via `chat::append`'s `file_xfer_id` check.

### 5.8 Code layout (planned)

- `src-tauri/src/mailbox/mod.rs` — types, envelope seal/open (`seal.rs`), key management (`keys.rs`).
- `src-tauri/src/mailbox/server.rs` — store, ACL, quotas, GC task (hourly expiry + orphan `.part` cleanup, reuse `gc_stale_partials` pattern), delivery loop.
- `src-tauri/src/mailbox/client.rs` — `reachability`, deposit/cancel/status, fetch on foreground.
- `iroh_net.rs` — dispatch `Some(kind) if kind.starts_with("mailbox.")` next to `locations.`; hello fields; `is_blockable_kind` gains `mailbox.deposit`.
- `chat.rs` — `held` status (keeps `status_rank` 0 so account-sync merges never override a real "delivered"; new `held_on: Option<String>` server eid field), `outbox()` filter excludes `held`; update the stale "no store-and-forward yet" header.
- `models.rs::Settings` — `transfer_server: {enabled, root, cap, expiry_days, access: "me"|"chosen"|"all", allowed: [friend ids], through: [friend ids]}`, `use_servers: [{eid, inbox, through}]`.
- `commands.rs` — `server_enable/disable/status/set_access/remove_user/wipe`, `servers_available`.
- UI: `src/components/TransferServerSettings.tsx`, `ServerExplainer.tsx` (animation), chat bubble status; iOS `TransferServerView.swift` (servers I can use + notifications), bubble glyph.

---

## 6. Push notifications (phase 2)

### 6.1 Options considered

| Option | Verdict |
|---|---|
| (a) Owner's Cloudflare Worker holds the APNs `.p8`, forwards opaque pushes; content encrypted end-to-end, decrypted on-device by a Notification Service Extension | **Recommended.** Works for everyone without per-user setup; Workers `fetch` speaks HTTP/2 to `api.push.apple.com` in production (confirmed by `cloudflare-apns2`, `paje`; note `wrangler dev` on macOS fails for APNs — test against a deployed worker). Free tier: 100k req/day. |
| (b) No push; deliver on next open | Phase-1 behavior and permanent fallback. Fine for desktops; poor for iPhones. |
| (c) Each server holds its own `.p8` | Impossible: the key is the developer's; can't be distributed. |
| (d) Silent background push (`content-available`) to let the app fetch | Throttled heavily by iOS (a few/hour, dropped in Low Power), app gets ~30s; iroh startup + dial is borderline. Use only as a *bonus* after the visible alert; never rely on it. |
| (e) Keep a socket alive (VoIP/PushKit) | App Store rejects PushKit for non-calls. No. |
| (f) Notification relay via a third party (ntfy/UnifiedPush) | Still needs APNs credentials for iOS; adds a dependency. No. |

### 6.2 Design of (a)

**Token handling (servers never see raw tokens):**
1. iOS app registers for remote notifications (`UIApplication.registerForRemoteNotifications`) → APNs device token.
2. App seals `{token, env:"prod"|"sandbox", bundle, allowed_server: S.eid, device: B.eid, exp: now+90d}` to the **Worker's X25519 public key** (baked into the app; rotateable via a key id) → `sealed_token`.
3. App sends `mailbox.push-register {sealed_token, push_key_pub}` to each inbox server it opted into. S stores it next to B's ACL entry. S cannot use it anywhere except through the Worker, and the Worker only honors it when the request is signed by `allowed_server`.

**Sending a push:** when S stores an item for B (or delivery fails because B is unreachable), S POSTs to `https://dropbeam-push.ashton-mcp-worker.workers.dev/push`:
```json
{ "v":1, "server": "<S eid>", "sealed_token": "...", "collapse": "<thread hash>",
  "payload": "<≤2 KB, sealed by the SENDER to B's push_key: {title, body, thread}>",
  "ts": 1790000000000, "sig": "<ed25519 by S over the above>" }
```
Worker: verify `sig` with `server` (Workers WebCrypto supports Ed25519), decrypt `sealed_token` (WebCrypto X25519 + HKDF + AES-GCM — or ChaCha via a tiny JS impl; pick AES-GCM for the worker-bound seal since WebCrypto lacks ChaCha20), check `allowed_server == server`, `exp`, rate limits in KV/Durable Object (per token 30/hour, per server 500/hour, global cap), sign an ES256 JWT with the `.p8` (cached 50 min), POST to APNs:
```json
{"aps":{"alert":{"title":"DropBeam","body":"New message"},"mutable-content":1,"sound":"default","thread-id":"<collapse>"},"e":"<payload>"}
```
Worker logs nothing but counters. On APNs 410/`BadDeviceToken` it returns `{gone:true}` and S drops that registration.

**Notification Service Extension (NSE):** decrypts `e` with the device's push key → replaces title/body ("Ashton", "running 10 min late" / "📎 3 photos"). If decryption fails or the user disabled previews, the generic "New message" stays. The *sender* seals the preview (A knows B's push key from hello), so S never sees text. A setting "Show message text in notifications" (default on) controls whether A includes the text or just "New message from Ashton".

**Feasibility with Tauri's generated Xcode project:** the iOS project is XcodeGen-driven (`src-tauri/gen/apple/project.yml`, committed). Add:
```yaml
targets:
  DropBeamNotify:
    type: app-extension
    platform: iOS
    sources: [NotificationService]
    info: { path: NotificationService/Info.plist, properties: { NSExtension: { NSExtensionPointIdentifier: com.apple.usernotifications.service, NSExtensionPrincipalClass: $(PRODUCT_MODULE_NAME).NotificationService } } }
    entitlements: { path: NotificationService/Notify.entitlements, properties: { keychain-access-groups: [$(AppIdentifierPrefix)com.ashtonmiller.dropbeam.shared] } }
    settings: { PRODUCT_BUNDLE_IDENTIFIER: com.ashtonmiller.dropbeam.notify, DEVELOPMENT_TEAM: R2RDA8476R }
  app_iOS:
    dependencies: [{ target: DropBeamNotify, embed: true }]
```
and to `app_iOS.entitlements` (currently empty): `aps-environment: production` (+ `development` for debug), same keychain access group. The NSE is pure Swift (~150 lines, CryptoKit only), no Rust, well under its 24 MB memory limit. Risk: `tauri ios init` regenerating the project — mitigated because `project.yml` is the source of truth and already hand-edited ("DropBeam additions"); document in `IOS-BUILD.md`. Needs a new App ID + provisioning profile for `.notify` (automatic signing handles it; TestFlight recipe in `dropbeam-testflight` memory stays the same).

Desktop recipients don't need push (the delivery loop reaches them when they wake; OS notification shows via existing `maybe_notify_chat`).

### 6.3 Owner's one-time setup (phase 2)

1. developer.apple.com → Certificates, IDs & Profiles → **Keys → +** → name "DropBeam Push", enable **Apple Push Notifications service (APNs)** (Sandbox & Production) → download `AuthKey_XXXXXXXXXX.p8` (one-time download), note Key ID + Team ID `R2RDA8476R`.
2. Identifiers → `com.ashtonmiller.dropbeam` → enable **Push Notifications**; register `com.ashtonmiller.dropbeam.notify` (extension).
3. Cloudflare: `wrangler deploy` the repo's `push-worker/` (name `dropbeam-push`) → secrets: `wrangler secret put APNS_KEY_P8`, `APNS_KEY_ID`, `APNS_TEAM_ID`, `WORKER_SEAL_PRIV` (generated by `node push-worker/genkey.js`, which also prints the public key to paste into the app build). KV namespace `PUSH_RL` for rate limits.
4. Send the agent the worker URL + seal public key; it goes into the app as a constant (settings override for self-hosters).
~20 minutes. Cost: $0 on the Workers free tier at this scale.

---

## 7. Security / threat model

Assets: message/file content, metadata (who talks to whom, when, sizes), server owner's disk and bandwidth, recipients' attention (push spam).

| Threat | Mitigation |
|---|---|
| Server owner (or thief of the box) reads content | E2E seal to recipient X25519 keys; server holds only ciphertext + header. Box theft reveals metadata only (who→who, sizes, times) for ≤ expiry window. |
| Server forges/modifies messages | Envelope signed by sender identity; recipient verifies against pinned `endpoint_id` + friendship; AEAD per segment. |
| Server drops/delays/replays | Can drop/delay (inherent). Sender sees "held" never turning "delivered" → after 48h shows "Still waiting on Linux Box" + "Send directly instead". Replays deduped (`mailbox-seen.json` + `chat::append`). |
| Stranger fills the server | ACL: deposits only from owner's account devices / allowed friends (`conn.remote_id()`); per-user quotas, rate limit, item cap, free-space floor; `block.rs` gate. |
| Allowed friend abuses "send through" to spam strangers | "Send through" is off by default and per-friend; recipients drop items from non-friends (and S deletes on `ok:false`); per-user item/recipient caps; owner sees per-user usage + can remove. |
| Recipient spoofing to steal items | `mailbox.fetch/get` only returns items whose `to` includes `conn.remote_id()`; even a wrong delivery is unreadable (sealed). |
| Key substitution (fake mailbox key) | Mailbox key accepted only with a valid signature from the pinned endpoint key; changes logged; key cached per endpoint. |
| Push abuse (spamming someone's phone) | Tokens sealed to the Worker and bound to one server eid; Worker verifies server signature + rate limits per token/server; B can revoke by unregistering (and tokens expire 90d). |
| Worker compromise | Attacker gets the APNs key + can send generic pushes to registered tokens and see server eids/timing; cannot read previews (sealed to device push keys) or messages. Rotate `.p8` (Apple allows revoke) + worker seal key id. |
| Revocation | Owner removes a user → their pending deposits deleted, future deposits refused (`rejected`), hello stops advertising access; their clients show "Linux Box is no longer available". Removing a device from the account (`account_remove_device`) → server stops delivering to that eid. |
| Disk exhaustion / NAS unmounted | Free-space floor; `.dropbeam-server-marker` + `locations::Root` mount identity → fail closed (refuse deposits, don't write to the root disk). |
| Old/compromised recipient device | Items sealed to every device in `person_endpoints` at seal time; removed devices drop out on next hello. Known residual: a device removed after sealing but before delivery could still decrypt that item — acceptable (same as it receiving it directly). |
| Forward secrecy | Per-item ephemeral X25519 gives sender-side FS; recipient key compromise exposes items still stored → rotate mailbox key monthly (keep previous key 30 days for in-flight items). |

---

## 8. Failure modes

| Situation | Behavior |
|---|---|
| Recipient offline for weeks | Items expire at server expiry (14d files / 30d chat). S records `expired`; A's next `mailbox.status` → bubble "Expired on Linux Box · Send again" (A still has the local source/message). Owner can extend per item. |
| Server offline when A sends | Deposit dial uses the same 4s probe; if S unreachable, fall back to today's direct outbox (keeps `failed`/retrying); try next usable server if any. |
| Server goes offline while holding items | Items wait on disk; delivery resumes on restart (index + GC scan). Recipients' `mailbox.fetch` fails silently. |
| Server disk full / over quota mid-upload | Reply `{ok:false, reason:"quota"}`; A cancels the deposit, shows "Linux Box is full", keeps direct retry. |
| A and B come online at the same time | Both paths may deliver. Chat: `chat::append` dedupe by id. Files: `item_id`/`file_xfer_id` dedupe on B (`mailbox-seen.json` checked *before* downloading from S → B acks "already have" and S deletes); if A's direct send completes first, A sends `mailbox.cancel`. Worst case = one duplicate download, never a duplicate message/file. |
| A deposits, then B connects directly to A | A's outbox sees B online; for chat it resends direct (cheap, deduped) and `mailbox.cancel`s; for files it lets S deliver (avoid re-upload) unless the item is still uploading. |
| Upload/download interrupted | Ranged resume (`have` ranges) on both legs; `.part` GC after 7 days (`PARTIAL_TTL_SECS`). |
| Multiple servers | A deposits to exactly one (first healthy in preference order); `item_id` is global so a retry to a second server after the first failed is safe (B dedupes). B fetches from all known. |
| Recipient's mailbox key unknown (old peer or never hello'd) | Can't seal → no deposit; direct only; UI: "Alex needs to update DropBeam to receive while offline". |
| Clock skew | Expiry uses server's clock; ordering uses `seq`; display uses sender `created_ms`. |
| Linux box reboots to a login screen (no GUI session) | Server down until login — phase 1 relies on autologin + autostart; phase 3 headless mode fixes it. |
| Push token expired / app deleted | APNs 410 → Worker returns `gone` → S deletes registration. |

---

## 9. Should the Transfer Server also be a relay?

- iroh supports custom relays (`RelayMode::Custom` in `iroh_net::start`); the current setting **replaces** the public relays and effectively requires both sides to configure it (comment in `start`). A better model: `RelayMap` with the owner's relay **plus** defaults, and advertise the relay URL in hello so friends add it too.
- Running `iroh-relay` on the Linux box needs: a public hostname (DuckDNS), router port-forwards TCP 80/443 + UDP 7842 (home router — CGNAT ISPs make this impossible), Let's Encrypt, and home **upload** bandwidth becomes everyone's relay bottleneck. `RELAY-SETUP.md` still pins `iroh-relay ^0.98` — must be updated to the 1.2 line to match the vendored iroh.
- The store-and-forward server largely removes the pain the relay was solving for *async* sends; for *live* sends the best fix remains an Oracle/VM relay per `RELAY-SETUP.md`.
- Cheap related win (phase 3): let the server bind a **fixed UDP port** (new setting) and document a single UDP port-forward, so senders reach the server *directly* instead of via canary relays — this improves upload speed to the server far more than running a relay.

Recommendation: not in v1; phase 3 optional "Also run a relay" checkbox that only appears when the wizard detects a public IPv4/IPv6 or a successful port-forward test.

---

## 10. Phased rollout + effort

Estimates are focused agent/dev days including tests + adversarial review, not calendar time.

### Phase 0 — spikes (2 days)
- Envelope seal/open in Rust (`ring` + `curve25519-dalek`), test vectors, CryptoKit XCTest decrypting the same vectors.
- Measure `reachability` probe timing Mac↔Linux↔iPhone (LAN, internet, relay-forced via Lab Mode) to validate the 4s budget.

### Phase 1 — owner's box stores-and-forwards chat + files for him and his friends (10–14 days)
- Server on desktop (Linux first; Mac/Windows work but warn about sleep): storage, index, ACL (me / chosen friends / all friends — **inbox for the owner + "send through" for chosen friends**), quotas, expiry, GC, delivery loop, `mailbox.*` kinds.
- Client: mailbox keys + hello advertisement, `reachability`, chat + op deposit, file deposit (resumable), fetch on start/foreground, dedupe ledger, `held` status + copy, `mailbox.status` polling.
- UI: desktop Transfer Server settings + wizard + management page, explainer animation, chat/transfer status; iOS: "Servers I can use", bubble status, fetch on foreground (no push yet).
- Account sync of `use_servers` so the owner's Mac/iPhone auto-use the box.
- Tests: loopback 3-endpoint harness (A, S, B in-process like `loopback_endpoint`), crash/resume, quota, expiry, revocation, race (direct + server), replay.
- Live test: Mac → Linux box → Mong's Mac / iPhone via Lab Mode.

### Phase 2 — iOS push (6–8 days + owner's 20-min setup)
- Push worker (`push-worker/`), seal key, rate limits; APNs key setup.
- iOS: remote-notification registration, NSE target in `project.yml`, keychain sharing, push key, opt-in UI + "show text in notifications".
- Server: `mailbox.push-register`, push on store, token GC.
- Optional: silent push → quick fetch (best-effort).

### Phase 3 — others' servers, polish, relay (8–12 days)
- Friends can run their own servers and share them (already mostly symmetric; add discovery UI + multi-server preference).
- Headless `DropBeam --server` mode + systemd unit + `.deb` post-install option ("Run as a background service").
- Fixed-port option, optional iroh-relay sidecar, relay map merge.
- Group sends (one ciphertext → many recipients), metadata padding, mailbox key rotation.
- Maybe: folder-sync catch-up via server (large design; separate doc).

Total ≈ 26–36 days across phases; Phase 1 alone delivers the core value.

---

## 11. Open decisions for the owner (each with a recommendation)

1. **Server choice rule** — deposit to the *recipient's* advertised inbox server first, else the sender's own/allowed server? **Recommend yes** (email-like; recipients who run a server get everything in one place and push works best).
2. **Default access for a new server** — "me only", "me + chosen friends", or "me + all friends"? **Recommend "me + all friends" for inbox** (friends can leave things *for you*) and **"send through" off by default**, granted per friend.
3. **Expiry defaults** — **Recommend 14 days for files, 30 days for chat**, owner-adjustable 1–90.
4. **Default storage cap** — **Recommend min(100 GB, 50% of free space)** with a slider; on the Linux box default the folder to the NAS if mounted.
5. **Per-item size limit** — **Recommend 20 GB** default (NAS-backed box can raise it).
6. **Offline probe budget** — **Recommend 4s for chat, 6s for files**, and treat iOS devices unseen for 120s as offline immediately.
7. **Metadata hiding** — pad sizes / hide chat-vs-file kind from the server? **Recommend no padding in v1** (it's your own/friends' box); revisit in phase 3.
8. **Notification text** — include message text in iOS pushes (decrypted on-device) by default? **Recommend on**, with a per-user "Hide message text" switch.
9. **Push relay hosting** — owner's Cloudflare Worker at `dropbeam-push.ashton-mcp-worker.workers.dev` holding the APNs key? **Recommend yes**; allow a settings override URL for self-hosters.
10. **Silent background fetch on push** — **Recommend best-effort only** (visible alert always; background fetch a bonus).
11. **Linux headless mode timing** — **Recommend phase 3**; for phase 1 enable GNOME autologin + DropBeam autostart on the box.
12. **Folder sync through the server** — **Recommend out of scope for v1**; chat + friend file sends only.
13. **Also run an iroh relay on the box** — **Recommend not in v1**; phase 3 optional, only when a port-forward test passes; prefer a fixed UDP port for direct uploads first.
14. **What happens to held items when the owner removes a user** — delete their deposits immediately vs. let them deliver? **Recommend deliver items already *to the owner*, delete everything else** immediately.
15. **Separate mailbox key vs converting the ed25519 identity** — **Recommend separate signed X25519 key** (rotation + iOS extension access without exposing the iroh identity).
16. **Name** — "Transfer Server" (owner's term) vs "Home Base"/"Drop Box". **Recommend keep "Transfer Server"** in settings, with the explainer's plain line "holds things for people who are offline".
