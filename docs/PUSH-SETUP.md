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
(sealed end to end — the server and Worker only see ciphertext).
