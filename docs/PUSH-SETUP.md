# Transfer Server push — owner setup (≈15 min)

Everything else is built and deployed: the relay Worker is live at
`https://dropbeam-push.ashton-mcp-worker.workers.dev` (check `/health`), its sealing
key is set, the app and the notification extension are in the build. Until you do
the steps below, iPhones simply get held messages the next time DropBeam opens.

**1. Create the APNs key** — developer.apple.com → Certificates, IDs & Profiles →
**Keys → +** → name `DropBeam Push`, tick **Apple Push Notifications service (APNs)**
→ Continue → Register → **Download** `AuthKey_XXXXXXXXXX.p8` (one-time download).
Note the **Key ID** (the X's).

**2. Enable Push on the App ID** — Identifiers → `com.ashtonmiller.dropbeam` →
tick **Push Notifications** → Save. (App Groups `group.com.ashtonmiller.dropbeam`
must also be on — the share extension uses it too.) The extension
`com.ashtonmiller.dropbeam.notify` is registered automatically by Xcode's automatic
signing on the next build.

**3. Give the Worker the key** (from `~/DropBeam-ios/push-worker`):

```sh
cd ~/DropBeam-ios/push-worker
wrangler secret put APNS_KEY_P8   < ~/Downloads/AuthKey_XXXXXXXXXX.p8
echo XXXXXXXXXX  | wrangler secret put APNS_KEY_ID
echo R2RDA8476R  | wrangler secret put APNS_TEAM_ID
curl -s https://dropbeam-push.ashton-mcp-worker.workers.dev/health   # → "configured":true
```

(`WORKER_SEAL_PRIV` is already set; its private half is in
`~/.private_keys/dropbeam-push-seal.txt`, public half baked into
`src-tauri/src/mailbox/push.rs`.)

**4. Turn the entitlement on and ship a build:**

```sh
cd ~/DropBeam-ios && scripts/enable-push.sh
```

Then build/upload to TestFlight as usual (bump `bundle.iOS.bundleVersion`).
The entitlement says `development`; Xcode switches it to production when it exports for TestFlight/App Store, so TestFlight phones use APNs production and `--debug` builds use sandbox automatically.

That's it. Phones register with the Transfer Servers they use; when a server holds
something for a phone that isn't connected it asks the Worker to send
"New message" and the phone's extension replaces it with the real sender and text
(sealed end to end — the server and Worker only see ciphertext; see below for
exactly what the relay and Apple can see).

## What the relay and Apple can see

A wake-up request carries:

- `sealed_token`: the phone's APNs token, sealed to the Worker. Only the Worker
  can open it, and only for the Transfer Server it was registered with.
- `collapse`: a 16-hex-char thread tag, `SHA-256("dropbeam-push-thread-v2" ‖
  phone ‖ sender)`. It is the same for every message from one sender to one
  phone, and it differs for every other phone, so it can't be used to link a
  sender across people.
- `payload`: `{"s": …}`. This is the sender's DropBeam device id plus the
  sender's sealed preview, sealed together to the phone's own notification key.
  The Worker and Apple see only ciphertext. The phone's Notification Service
  Extension opens it, checks the preview really came from that sender, and
  titles the banner with your own name for them.

The Worker also sees the signing server's public key and the caller's IP. It
logs only APNs status codes. Apple sees the device token (it has to, to deliver)
and the generic alert "DropBeam / New message", which the extension replaces
on the phone.

**One exception.** If a Transfer Server doesn't know the phone's notification
key yet, it sends the older `{"f": sender id, "e": sealed preview}` form, and
then the sender's device id is visible to the Worker and to Apple. This happens
when the phone's signed key hasn't reached the server, either in the push
registration or in a hello. On current builds the server learns the key from
the phone's hellos (own devices and friends). A phone running a build from
before this change can't open the sealed form: it gets the generic
"DropBeam / New message" banner until it updates.

## Blocked senders

- **Your own Transfer Servers** sync your block list with your account, and they
  never send a push for an item from someone you blocked.
- **A friend's Transfer Server** doesn't know your block list. Block lists are
  never shared outside your own devices. There, the phone's extension checks
  `push-blocked.json`, which the app keeps in the App Group. Without Apple's
  filtering entitlement it turns the push into a silent, passive "DropBeam /
  New activity" with no name, no text and no sound. To drop such pushes
  entirely, request `com.apple.developer.usernotifications.filtering` from Apple
  (developer.apple.com/contact/request/notification-service). Once it's granted,
  add it to `DropBeamNotify.entitlements` and set the Boolean `DropBeamCanFilter`
  = YES in `DropBeamNotify/Info.plist`.

## Redeploying the Worker (rate limits)

The Worker no longer uses KV. Per-request KV writes let anyone burn the daily
write quota and stop push for everyone. It now uses Workers Rate Limiting
bindings, all keyed on values the Worker derives itself: client network (an
IPv4 address, or an IPv6 /64 so one host can't rotate addresses), global,
verified server key, and a hash of the decrypted phone token. The global limit
is charged only after a request passes the signature check, opens a token
sealed for that server, and passes its per-server/per-phone limits, so forged
or junk requests can't use it up. The bindings are in `wrangler.toml`. To deploy:

```sh
cd push-worker && npx wrangler deploy
curl -s https://dropbeam-push.ashton-mcp-worker.workers.dev/health
```

Once that works, the old `PUSH_RL` KV namespace can be deleted
(`npx wrangler kv namespace delete --namespace-id 01482baf090e4b7c994c4010d5e251b7`).
