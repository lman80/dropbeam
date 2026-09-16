# Feedback triage — open GitHub issues

One pass over the open issues in `lman80/dropbeam`, checking the **current** code
first. Every verdict below is either a fix landed on this branch, an
"already fixed" with a file:line citation, or a design note for something
deliberately not implemented.

Issues **#24, #25, #29, #30, #33** are owned elsewhere and are not covered here.

---

## Verdicts

| Issue | Verdict |
| --- | --- |
| #34 presence stuck on "offline" until restart | **Fixed** |
| #27 unread badge never clears ("17 new") | **Fixed** |
| #26 chat text must be selectable/copyable | **Partly already fixed → completed** |
| #17 URLs in chat clickable | **Partly already fixed → hardened + extracted** |
| #22 nested empty subfolder failed to send | **Round-trip already correct** (+ a real nested-specific bug found and fixed, + test) |
| #28 / #19 / #18 lost a friend's contact after update | **Already fixed** — guards and regression tests present |
| #23 all interactions should appear in chat | **Already fixed** — all three sub-asks are implemented |
| #16 "testing the feedback feature" | **Close** — a test post, no action |
| #12 who-added-this-file badge | **Design only** (below) |
| #20 auto speed-limit detection | **Design only** (below) |
| #31 one account across devices | **Design only** (below) |

---

## #34 — "It said offline when it wasn't and it wouldn't recheck" — FIXED

Root cause was that nothing ever re-checked a friend once they went quiet. The
control-beacon prober backs off to 60 / 120 / 300 s, and `friendPresence`
(`src/lib/presence.ts`) treats anything older than a 2-minute window as offline,
so a friend who came back could read "offline" indefinitely. Worse,
`LocationsView` only asks friends it *believes* are online for their locations,
so a stale "offline" also hid their folders.

Three changes:

1. **Accepting a connection now marks the friend online immediately** —
   `src-tauri/src/iroh_net.rs:1717-1723`. `handle_conn` previously waited for the
   45 s `presence_tick`; the accepted connection's `remote_id()` is the
   authenticated transport identity, so it is proof of life the moment they dial.
2. **Opening a view that shows presence actively re-checks** — new
   `claimPresenceChecks` in `src/lib/presence.ts:69-115`, wired into
   `src/views/FriendsView.tsx:30-36`, `src/components/SendToChooser.tsx:26-33`
   and `src/views/LocationsView.tsx:147-152`. It returns only friends that have a
   device address and don't already read as online, and stamps them, so three
   views opening in a row produce **one** ping per friend (20 s cooldown) rather
   than a dial storm.
3. **Manual "Check now" was already there** — `src/views/FriendsView.tsx:367-381`
   (`check` → `store.pingFriend` → `markFriendSeen`), button at
   `src/views/FriendsView.tsx:521-537`. Left as is.

Tests: `tests/presence-recheck.test.ts` (4).

## #27 — "It always says 17 new notification" — FIXED

Two real gaps in `src/store.ts`:

* `markChatRead` returned early on `!windowFocused` **before** clearing the local
  badge, so opening a thread while the window didn't report focus left the count
  stuck. The focus gate now guards only the outbound read receipt.
* `chatUnread` was memory-only, so the badge was rebuilt from scratch and a clear
  never stuck. It is now persisted to `localStorage` with shape validation and
  pruning to live friend ids, written through a single `saveChatUnread` point
  that also pushes the Dock/taskbar badge. Overlay webviews never write (their
  copy of the map is frozen at their own startup).

## #26 — selectable / copyable chat text — COMPLETED

`.chat-bubble { user-select: text }` already existed (`src/index.css:757-762`).
Added the `-webkit-user-select` twin (the macOS WKWebView only honours the
unprefixed property from Safari 17.4 — a real gap on older Macs), made
`.chat-quote-text` selectable, and added a **Copy** action to the per-message
menu, which now also opens on *incoming* bubbles (it was owner-only, so a
friend's message could never be copied).

## #17 — clickable links in chat — HARDENED

An inline linkifier already existed in `ChatView`. It is now the pure
`src/lib/linkify.ts`: `http`/`https` only (no blocklist to bypass — `javascript:`,
`data:`, `file:`, `mailto:` and bare `www.` simply never match), balanced-bracket
trailing-punctuation trim, a host requirement and a 2048-char cap. Anchors carry
`rel="noopener noreferrer"`, call the opener plugin, never navigate the webview,
and nothing uses `dangerouslySetInnerHTML`. Tests: `tests/linkify.test.ts` (8).

## #22 — nested empty subfolder — ALREADY CORRECT, plus one real fix

The mirror/total-sync path does carry empty directories:
`live_empty_dirs` (`src-tauri/src/sync.rs:3859`) emits **every** directory it
walks minus those that contain files, so `a/b` yields both `a` and `a/b`; it
rides the beacon as `emptyDirs` (`sync.rs:1537-1539`, decoded at
`iroh_net.rs:3014-3018`) and the receive side uses `create_dir_all`.

One nested-specific bug *was* found: the apply loop only skipped a rel if **that
exact rel** was tombstoned, so `create_dir_all("a/b")` could silently resurrect a
deleted `a`. It is now `apply_empty_dirs` (`sync.rs:3915`, called from
`sync.rs:2143`), which checks every ancestor prefix against both tombstone maps.
Test: `sync::tests::nested_empty_dir_round_trips_to_the_peer` (`sync.rs:5046`).

**Caveat to pass back to the reporter:** this covers the mirror beacon, which is
the only path carrying `emptyDirs`. A plain Quick Send / friend send walks
`list_files_rec` and has no directory concept at all, so if #22 was reported
against a non-mirror send that is a separate gap worth reproducing.

## #28 / #19 / #18 — lost a friend's contact after an update — ALREADY FIXED

All three v0.38 guards are present in `src-tauri/src/friends.rs` and unmodified:

* `.bak` sidecar + recovery from a corrupt read — `read_raw` (`friends.rs:37`) →
  `settings::read_json_array_resilient` (`settings.rs:50`; retries transient IO
  4×, falls back to the `.bak`, and refuses an **empty** `.bak` so a deliberate
  "removed my last friend" is not undone). Note the file is `friends.bak`, not
  `friends.json.bak` (`Path::with_extension` replaces the extension).
* Empty-clobber guard — `friends.rs:62-73`: writing an empty list over a
  non-empty one is refused and logged. Only the deliberate-removal path passes
  `allow_empty`.
* Dedup / phantom-contact — `reconcile` (`friends.rs:315`) over the pure
  `plan_reconcile` (`friends.rs:238`), plus `self_heal_chat_sender`
  (`friends.rs:503`).

Regression tests already exist, so none were added:
`save_refuses_to_clobber_with_empty_list` (`friends.rs:1187`),
`load_recovers_from_backup_when_primary_corrupt` (`friends.rs:1202`),
`empty_clobber_guard_holds_when_primary_unreadable` (`friends.rs:1222`),
`remove_last_friend_is_not_resurrected_by_backup` (`friends.rs:1235`).

The reporter is on **Windows at v0.25 / v0.34**; the fixes shipped in v0.38, so
they must update.

## #23 — all interactions should appear in chat — ALREADY FIXED

All three things the issue asks for exist:

* **Direct sends become chat rows** — `store.sendToFriend`
  (`src/store.ts:1152-1182`) posts a synced file note and links the transfer to
  the card. Every entry point funnels through it (window drop `App.tsx:73` →
  `SendToChooser.tsx:30-33`, Send view `SendView.tsx:37`, friend card
  `FriendsView.tsx:387`, menu-bar popover). The native tray drag-send bypasses
  the JS store and is covered in Rust by `post_file_note`
  (`src-tauri/src/commands.rs:1311-1316`). Quick Send by link/QR correctly posts
  nothing — there is no friend.
* **Drag onto the text box, stage, send with the text** — `chatDraftFiles` +
  `stageChatFiles` (`src/views/ChatView.tsx:251-253, 534-539`), with paste-image
  support at `ChatView.tsx:562`.
* **Shared-folder activity in chat** — `folderActivity` rows rendered inline in
  the thread (`ChatView.tsx:246, 387-415, 934-977`), including moves.

## #16 — CLOSE

"Testing out the send back feedback feature." A deliberate test post from
v0.15.0. No code action; close it.

---

## Design notes (not implemented)

### #12 — show who added a file, like Blip

> "when a file or folder is added from DropBeam in a shared folder, it showed who
> it was from, like a little icon on the actual file"

**The data layer already exists.** `src-tauri/src/provenance.rs` stamps the
sender onto every received file as the macOS extended attribute
`com.dropbeam.from`, and shared-folder receives call it at `sync.rs:2207`. What
is missing is the *display*, and the two halves of the ask need different work:

*Inside DropBeam* is the cheap half and should ship first. Add a
`files.provenance(paths) -> Record<path, sender>` command that reads the xattr in
one batched `spawn_blocking` walk (never per row, or a NAS listing turns into
hundreds of round-trips), and render it in the Locations `FileBrowser` and the
History "Recents" tab as a small avatar chip in the name cell, reusing
`FriendAvatar`. Files with no xattr — anything that predates v0.38, anything
copied in by hand, and everything on Windows/Linux — must render with **no** chip
rather than "Unknown"; a wrong attribution is worse than none. Store the
friend's stable id alongside the display name so a later rename still resolves.

*In Finder*, which is what the screenshot actually shows, needs a **Finder Sync
extension**: a separate app-extension target inside the `.app`, sandboxed, with
its own entitlement, that registers the shared-folder roots as directory URLs and
returns a badge per file from the xattr. That is a meaningful build-and-signing
change (extension target, provisioning, notarization of a nested bundle), the
user must enable it once in System Settings → Extensions, and badge icons are
limited to a small registered set — so "the friend's avatar" is not possible,
only a generic "from a friend" badge, optionally one per friend up to the badge
limit. It is also macOS-only, with no Windows equivalent short of a shell
extension DLL. Recommend shipping the in-app chip, then treating the Finder
extension as its own project with its own release checklist.

### #20 — detect a safe speed limit instead of guessing

> "I don't know what the top speed of my router is without crashing it. So I just
> put 150. I wish there was a way that we could test it and recommend a default."

The real failure is not raw throughput — it is **NAT/conntrack table exhaustion
and buffer-bloat on a cheap home router**, which is why the router falls over
rather than merely slowing down. So a calibration that only measures MB/s would
recommend a number that still crashes the router.

Design a **"Find my safe speed" calibration** run against a friend's device (or
the relay when no friend is up), in the existing Lab-mode style, as a one-off the
user starts from Settings → Speed limit:

1. Ramp the upload limiter in steps (e.g. 5 → 10 → 25 → 50 → 100 → 200 MB/s),
   holding each step ~10 s.
2. At each step record what we already measure: achieved MB/s, RTT and loss from
   `probe_connection` / `ConnDetail`, and whether the path stays **direct** or
   falls back to relay.
3. Stop at the first step where RTT more than doubles against the idle baseline,
   loss appears, the path degrades, or a step fails to increase throughput.
4. Recommend the **previous** step with a safety margin (≈70 %), and show the
   evidence: "Your link stayed healthy to 90 MB/s and stalled at 120 — we suggest
   85 MB/s." Offer Apply / Keep mine.

Ship it as a recommendation, never an automatic change: the whole point is that
the user's router is the fragile part, and silently raising the cap risks exactly
the crash the feature exists to avoid. Two extra rails worth having: run it only
on a direct path (a relay measures the relay, not the router), and detect a
mid-run collapse — if RTT spikes and *stays* spiked after the ramp stops, the
router is already in trouble, so abort, drop to the lowest step and say so.

### #31 — one account across devices, with shared progress

> "if you had the same account on your phone and your computer … you could see
> the progress of whatever is being sent on the other device"

This is the largest of the three, because DropBeam's identity model is
deliberately **per-device**: a friend *is* an iroh `EndpointId`, and
`locations::authorize` treats "the authenticated transport identity is the ONLY
authority" as a security invariant. An "account" spanning devices must not become
a shared secret that weakens that.

The shape that fits the existing architecture is **a device group you own, not an
account on a server**:

* **Identity.** Keep one iroh endpoint per device. Add a signed *device-group*
  record: each device generates its own keypair, and a new device joins by
  scanning a QR from an existing one (the pairing flow already exists). The group
  is the set of endpoint ids that have signed each other in. No password, no
  server, nothing to breach — and `authorize` keeps checking endpoint ids, now
  against "any endpoint in my friend's group" instead of one.
* **Contacts and locations.** Friends and hosted locations become group-scoped:
  adding a friend on the laptop syncs the contact to the phone over the same
  control channel that already carries the roster beacon. This alone would have
  prevented a good share of #18/#19/#28-style "I lost my contact" reports.
* **Shared progress.** Mirror the `TransferUpdate` stream to group peers as a
  compact read-only digest (id, direction, peer name, bytes, state) on the
  existing control channel, and render the other device's transfers in the Send
  view under a "On your other devices" group, greyed and non-cancellable at
  first. Cross-device *cancel* is a later step and needs an explicit ack, since
  acting on a remote device's transfer is exactly the kind of authority we
  otherwise never grant.
* **What it costs.** Every place that assumes "one device = one identity" has to
  be revisited: friends.json dedup, `Pair.owner_eid` and the role epochs, the
  location mount markers, chat threads keyed by friend id. It should be a
  versioned protocol bump with a fallback for single-device peers, not a patch.

Worth noting it also depends on the iOS build shipping — the value the issue
describes is specifically phone ↔ computer.
