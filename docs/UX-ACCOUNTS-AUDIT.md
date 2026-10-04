# Grandma walkthrough: accounts, my devices, friends and messages

Branch `ux-accounts` (from `ios` @ 74aaaf1), 2026-10-05. Walked as a 75-year-old who has
never heard of peer-to-peer, on the desktop app (Vite mock, macOS look) and the native
iPhone app (code + simulator). Before/after screenshots: `/tmp/dropbeam-audit/ux-accounts/`.

**P1** = she gets stuck or loses something. **P2** = confused, but recovers. **P3** = polish.
Status: **Fixed** (this branch), **Design** (needs an owner decision, see the end),
**OK** (already good), **Deferred**.

## 1. First run — "what is my account, do I need one?"

| # | Where | Finding | Rank | Status |
|---|---|---|---|---|
| 1.1 | Desktop first-run dialog | The only way in for a second device is a link button called "Link an Existing Device…". She doesn't know which device is "existing". iPhone already says "I Already Use DropBeam". | P2 | Fixed: "I Already Use DropBeam…" on desktop too |
| 1.2 | Both | The app never says what an "account" is, yet Settings and errors talk about "Remove This Device from Account" and "different accounts". To her there's no account, just "my devices". | P2 | Fixed: user-facing copy now says "your devices" / "My Devices" ("Remove This Mac from My Devices…", "Both devices are already set up with different people's devices…") |
| 1.3 | Both | Nothing says the iPhone and the Mac can be "the same me". The Devices page explains it only after linking. | P2 | Fixed: Devices page (desktop + iPhone) opens with one sentence: "Link your phone and computers so they're all you: same friends, same chats, same name and photo." |
| 1.4 | Both | No sign-in, no email, no password. Good for her, but there is also no recovery (see 3.3). | — | Design D1 |

## 2. Linking my second device

| # | Where | Finding | Rank | Status |
|---|---|---|---|---|
| 2.1 | Desktop link dialog | Instruction "open Settings → Devices → Link a device and scan this code" — but on the other device, Link a Device opens its OWN code. Both screens end up showing a QR code at each other; nobody scans. Dead end. | P1 | Fixed: numbered steps that end in "tap **Scan**", and the other device's screen now leads with "Scan the other device" on phones |
| 2.2 | Both | Safety-code screen has only "Codes Match — Link" and "Cancel". No "they don't match" choice; she doesn't know what a mismatch means or what to do. | P1 | Fixed: "Look at your other device. Does it show the same 6 numbers?" + three choices: **Yes, They Match**, **No, They're Different** (cancels, explains "nothing was linked — someone else may have scanned your code; start again with the devices side by side"), Cancel |
| 2.3 | Both | Safety warning text is long and legalistic ("full access to your account: your friends, chats and devices"). | P2 | Fixed: one short line: "Only link your own devices. Linking shares all your friends and chats with it." |
| 2.4 | Both | Timeout: code lives 10 min (link.rs TTL). Expired-code error is plain and offers Try Again. | — | OK |
| 2.5 | Both | Camera denied: desktop scanner falls back to Paste/screenshot with a System Settings path; iPhone shows "Allow Camera". | — | OK |
| 2.6 | Both | Other device offline/closed: "Couldn't reach your other device. Make sure DropBeam is open on it…" + Try Again. | — | OK |
| 2.7 | Both | Already in different accounts: error said "Settings → Devices → Remove This Device from Account" — the label doesn't exist with that wording on any platform. | P2 | Fixed: wording matches the real button ("Remove This … from My Devices") |
| 2.8 | Desktop | Both copies of a device with the same name read "Your iPhone 1 / Your iPhone 2" with no way to tell which is which except "Online / Offline". | P3 | OK (subtitle shows the device's own name when they differ) |
| 2.9 | Both | "Needs approval" device: "Needs approval — only approve it if it's yours." Doesn't say why it's there or what Remove does. | P2 | Fixed: "Says it's one of your devices, but none of your devices added it. If you don't recognize it, tap Remove." + confirmation before Approve |
| 2.10 | iPhone | While linking, the toolbar button reads "Hide" (audit Wave 6: "Hide cancels device linking"). | P3 | Fixed: "Cancel" (it does cancel) |
| 2.11 | Both | Third device: works the same (join code from any linked device). | — | OK |

## 3. Losing / replacing a device

| # | Where | Finding | Rank | Status |
|---|---|---|---|---|
| 3.1 | Both | Removing a lost phone works (⋯ → Remove), confirmation says what it means. But nothing on the page tells her that's what to do when a phone is lost. | P2 | Fixed: Devices footer "Lost a phone or computer? Remove it here. It stops getting new messages; what's already on it stays on it." |
| 3.2 | Both | New phone: link it like any other device — friends/chats come along. | — | OK |
| 3.3 | Both | Only device lost (or all of them): everything is gone, friends must be re-added. Nothing says so. | P1 | Design D1 (recovery). Copy fixed: Devices page says "Your friends and chats are only on your devices — link a second one so losing one doesn't lose them." |
| 3.4 | Both | "Remove this Mac from account…" (leave): consequence copy is OK, label is jargon (see 1.2). | P2 | Fixed (1.2) |

## 4. Adding a friend

| # | Where | Finding | Rank | Status |
|---|---|---|---|---|
| 4.1 | Both | **After I add Alex by his code, Alex gets a friend request he must accept — but my screen says "You added Alex" / "Connected", and my messages sit at "Sending…" forever with no reason.** | P1 | Fixed (engine + UI): the hello reply now says whether they accept us; until they do, the friend reads "Waiting for Alex to accept" (row, chat header note, iPhone add-friend screen), and messages say they'll arrive once Alex accepts |
| 4.2 | Desktop | A friend request arrives silently: no sidebar badge on Friends, no notification. She never opens Friends. | P1 | Fixed: Friends sidebar badge + a system notification ("Jordan wants to be your friend") that opens Friends |
| 4.3 | iPhone | Requests show a tab badge but no notification. | P1 | Fixed (same engine notification; tap opens Friends) |
| 4.4 | Desktop | Request row is broken: name and explanation run together ("JordanWants to be your friend…"), long sentence. | P2 | Fixed: two lines, short subtitle |
| 4.5 | Desktop | "Copy" on your code copies a bare `dropbeam:eyJ…` string. Pasted into a text message it means nothing to the person receiving it. iPhone shares a friendly message with a tap-to-add link. | P2 | Fixed: desktop "Copy Invite" copies the same friendly message + link the iPhone sends |
| 4.6 | Both | Friend appears on all my devices; one person with several devices is one row. | — | OK |
| 4.7 | Both | Same-name friends: look-alike hint exists ("might be the same person…"). | — | OK |
| 4.8 | Desktop | Remove friend confirmation: "Your chat history stays on this device." It's actually removed from ALL my devices. | P2 | Fixed: "Alex is removed from all your devices. Your messages stay." |
| 4.9 | Both | Block/unblock: clear dialogs; unblock in Settings. | — | OK |
| 4.10 | Both | Renaming a friend: "Only you see this name" on iPhone; desktop no hint. | P3 | Fixed: desktop hint |

## 5. Messaging

| # | Where | Finding | Rank | Status |
|---|---|---|---|---|
| 5.1 | Both | Offline: "Alex is offline. Messages will send when you're both online with DropBeam open." / "…wait on Linux Box". Good. | — | OK |
| 5.2 | Both | Status words: Sending… / Waiting to send / Delivered / Read / "Held on Linux Box — reaches Alex when they're online". "Held on" is odd. Nothing explains a status if she's unsure. | P2 | Fixed: "Waiting on Linux Box — Alex gets it when they're back"; clicking/tapping the status explains it in one sentence |
| 5.3 | Both | Failed message: outbox retries forever, file cards have Retry. | — | OK |
| 5.4 | Both | **Reading a chat on the Mac leaves the iPhone's unread badge up (and vice versa); a message that reached the Mac and synced to the iPhone never counts as unread there.** | P2 | Fixed (engine): own devices share how far each chat has been read; reading on one clears the badge on the others, and synced-in new messages count as unread |
| 5.5 | iPhone | Notification for a chat read elsewhere stays in Notification Center. | P3 | Fixed: clearing a chat's unread also clears its delivered banners |
| 5.6 | Both | Sent from phone shows on Mac: yes, via own-device sync (seconds when both online). | — | OK |
| 5.7 | Both | Edit/unsend/react: right-click/hover (desktop), long-press (iPhone). Unsend is immediate, no confirmation. | P3 | Deferred |
| 5.8 | Both | Group chat: not offered, not implied. | — | OK |
| 5.9 | Both | Presence: "Last seen 3h ago" / "Not seen yet" / "Connecting…". OK; folder presence still keyed by name (audit P2). | P3 | Deferred (presence.ts) |

## 6. Edge cases

| # | Finding | Rank | Status |
|---|---|---|---|
| 6.1 | Clock skew: chat ordering is Lamport-style; read markers compare the friend's own message timestamps (max-merged), so skew between my devices can't resurrect unread. | — | OK |
| 6.2 | Two of my devices sending at once: both messages kept, ordered by time. | — | OK |
| 6.3 | Friend removes me: my device now reads "Waiting for Alex to accept" (honest: messages won't arrive) rather than "Sending…" forever. Blocked people are answered like before (no new signal), so a block still isn't revealed. | P2 | Fixed (4.1) |
| 6.4 | Very long / emoji / RTL names: truncated with tooltip on desktop; names are sanitized (bidi/zero-width) in the engine. | — | OK |
| 6.5 | Version mismatch: link screen says "(needs an update to show the code)"; old friend versions ignore new fields. | — | OK |
| 6.6 | Transfer Server offline: bubble note via serverNoteText. | — | OK |
| 6.7 | Device removed while sending: its queued messages are dropped from its outbox (design-my-devices). | — | OK |

## Owner decisions

**D1 — Recovery when every device is lost.** Today there is none: the account key lives only
on devices, there is no server. Options:
1. *Recovery kit* (recommended): at link time, offer "Save a recovery code" — 24 words or a
   printable QR containing the account key encrypted with a random secret. Restoring on a new
   device re-creates the account and asks surviving friends to re-confirm. Engine work:
   export/import of the account key + a "restore" link direction (~1 week incl. UI/tests).
2. *Encrypted backup on a Transfer Server* the user owns (fits "rent our stuff"): the key and
   friends list sealed with a passphrase, stored on their server. Depends on Transfer Server.
3. *Do nothing, say it plainly* (shipped now as copy): "Your friends and chats are only on
   your devices — link a second one so losing one doesn't lose them."

**D2 — Friend requests from people who added you.** Today adding by code is one-sided until
the other person taps Accept (S2 security fix). An alternative that keeps S2's protection: a
friend code could carry a per-code secret so that someone who scanned *your* code is accepted
automatically (they proved they got it from you), while a hello from anyone else stays a
request. Codes stay reusable; changing them invalidates the old secret. Needs a code-format
bump (v2) — design only.

**D3 — Notify on every device vs. one.** Messages via a Transfer Server ring every device
(iMessage-style); messages sent directly reach one device and sync to the others silently.
With 5.4 the badge is right everywhere; whether the synced-in device should also show a
banner is a product choice (risk: late banners for old messages).

## Deferred (P3)
- Unsend confirmation / undo (5.7).
- Presence by friend id for shared-folder status (5.9).
- Look-alike hint could offer "These are the same person" merge (needs account proof).
