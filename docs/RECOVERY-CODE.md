# Recovery code

Owner decision D1 / item 3.3 in [UX-ACCOUNTS-AUDIT.md](UX-ACCOUNTS-AUDIT.md): when every
device is lost, the person should not become a stranger to everyone. There is still no
server and no password: the account *is* its key, and the recovery code is that key on paper.

Code: `src-tauri/src/recovery.rs` (+ `recovery/tests.rs`), small hooks in `link.rs`,
`account.rs`, `friends.rs`, `iroh_net.rs`. Desktop: `src/components/RecoveryCode.tsx`,
`src/lib/recovery.ts`. iPhone: `UI/RecoveryViews.swift`, onboarding step, Devices section.

## What the person sees

- **Save your recovery code.** Offered once right after first-run setup ("Later" is fine),
  always in Settings → Devices. Why → the 12 words (plus a QR and
  Print…) → "Which word is number 6?" twice, four choices from the code → saved.
  Warnings in plain words: "Anyone with these words can become you. Keep them somewhere
  safe, like with your important papers." / "DropBeam will never ask for them…".
  Settings shows *Saved* or *Not saved yet* (amber).
- **Restore with Recovery Code** on the welcome screen of a new device (desktop link under
  the name field; iPhone welcome page). One box per word, the first four letters are
  enough, paste or scan the printed QR fills all boxes, a mistyped word is marked by number,
  a swapped word fails the checksum ("one of them is probably mistyped").
- **After a restore** (Settings → Devices, "Since you restored"): how many friends found
  you again and who; *your devices from before* (each: Remove… / I Still Have It; Remove
  All…); *shared folders you were in* (name + which friend) so you can ask to be invited.

## Decisions

**Words.** BIP39 English list and checksum (catches typos and swapped words).
- Accounts minted by this build: 16 random bytes → **12 words**; the account key is
  `SHA-256("dropbeam-account-from-words/1" ‖ seed)` (domain-separated, so a wallet's words
  give an unrelated key). The seed is kept in `account.seed` (0600) beside `account.key`
  and travels in link offers (`account_words_hex`), so every device of the account shows
  the same words. A seed is only trusted while it still derives the key on disk.
- Accounts that already exist (random 32-byte key): **24 words** = the key itself. Their
  key cannot be shortened, and switching to a new key would make every friend lose track
  of the person (friends recognize people by account key). A device linked from an older
  build has no seed and shows the 24-word form of the same key — both restore the account.
- A device with no account yet (single device, never linked) gets one when the code is
  first shown; friends learn the account from the next hello.

**New device key, not a derived one.** Restoring installs the account key under a **new
endpoint (device) key**. Deriving the endpoint key would give the restored device the lost
device's id: if that device turns up (or a thief has it), two live devices share one id —
connections fight, and friends can't tell them apart or revoke one. Old endpoint keys were
random anyway. A new endpoint that proves the account is exactly what every build already
treats as "another device of this friend" (`HelloOutcome::AddedDevice`).

**Own devices after a restore.** The account book starts fresh with the restored device as
its only member (link proofs on, S4). Any device from before that shows up proving the key
is listed as *Needs approval*. Devices friends still know are listed as *from before*;
*Remove* signs a removal with the restored device's key, which friends accept (the signer
proves the account in the same hello), so a lost or stolen device stops speaking as the user.
Nothing is removed automatically: someone who restores while an old Mac is merely switched
off at home shouldn't have it kicked out without being asked.

**Finding each other (rendezvous).** The new device knows nobody, and friends only know the
lost device ids. A restored device publishes, for 30 days (every 6 h), a signed pkarr record
on n0's pkarr relay — the service iroh already publishes device addresses to — **under the
account key**: TXT `_dropbeam` = `v=1`, `e=<device id>` (this device + its own devices, ≤ 4).
Every device looks up its friends' account keys every 3 h (first ~3 min after start) and
greets any device listed there that it doesn't know yet. Only someone who knows the
account's public key (friends, own devices) can look it up; a record older than 60 days, or
not signed by that account, is ignored. Expect friends to find a restored device within a
few hours; a friend can also just re-add you by friend code at once.

**Vouches: former friends come back without a request.** In every hello (and hello reply)
between friends, each side includes its account's signature over the *other's* device id
(`vouch`, message `dropbeam-friend-vouch/1|<eid>`), which the other keeps per account
(`friend-vouches.json`). A friend greeting a restored device presents the vouch it holds
(`your_vouch`); it verifies against the recovered key, so that friend is added back at once
(`HelloOutcome::Returned`) and the restored device greets them back. A stranger can't make
such a vouch (it needs the account key) and can't reuse one (it names the friend's device id,
which the iroh connection authenticates); a hello without a valid vouch is a normal friend
request (S2).

**Chat history comes back from friends (`restore-sync`).** For each friend who is back, the
restored device asks once (with backoff) for their copy of the conversation. The friend
answers only a device that proves the account of someone it *already* has as a friend (not
revoked, not blocked; rate-limited per device): the newest 500 messages (≤ 8 MB), the
person's other device ids it knows, and the **names** (never paths) of folders it shares
with them. The restored side flips who-sent-what, merges like an own-device sync (no
notifications, no unread), and records the old devices and folders. Files and folder
contents are not recovered.

**Security.**
- The words are the account. They are never logged, never sent over the network (only
  between the engine and the screen), and wiped from Rust memory after use (`zeroize`);
  JS/Swift strings can't be wiped but are dropped as soon as the screen closes.
- A restore can't take over anything the key doesn't already imply: friends already accept
  any device proving the account key (that's how a second device works). History is only
  answered to such a device, by friends who already know the account.
- Restore is refused when the device already shares a *different* account with other
  devices ("remove this device first"), and when it already holds this account.
- Known residual: vouches can't be revoked. A friend you removed or blocked *before* losing
  every device (the block list is lost with them) comes back after a restore if their app
  still holds your vouch. They are listed under "found you again"; remove or block again.
- Rendezvous lookups send friends' account keys to n0's pkarr relay (PRIVACY.md).

**Older versions.** Nothing changes for them; unknown hello fields are ignored and
`restore-sync` to an old build just fails quietly. An old-version friend never looks up the
rendezvous record, so the person re-adds them by friend code — the old app then recognizes
the account (AddedDevice) automatically. Chat history comes back only from updated friends;
vouches only exist once both sides ran this build at least once.

## Tests

`cargo test --lib recovery::` — BIP39 reference vectors; forgiving input (numbering,
case, 4-letter prefixes, QR form); checksum / unknown-word messages; 12-word mint → restore
gives the same account and words; 24-word legacy key; stale seed never shown; restore
refusals; saved-confirmation bound to the current account; restored book makes old devices
need approval; vouch brings a friend back and nobody else; restore-sync flips the
conversation and lists old devices + folder names (not paths); non-friends and forged proofs
get nothing; rendezvous record signature/age checks; lookup targets; restore-sync over a
real loopback iroh connection. `tests/recovery.test.ts` — quiz, splitting, labels, printed sheet.

## Needs live testing

1. Desktop + iPhone: save flow (print on macOS/AirPrint, quiz), restore flow incl. QR scan.
2. Two machines: P (Mac) and F (friend) chat; wipe P (or use a fresh config dir) and restore
   on another device → within ~3 h (or F restart + ~3 min) F greets it, F appears as a
   friend without a request, history arrives, P's old Mac is listed "from before"; Remove it
   → F stops treating it as P.
3. pkarr publish/resolve against dns.iroh.link (HTTP 200 on PUT, record resolves).
4. Mixed versions: old friend re-added by code is recognized; nothing breaks for them.
