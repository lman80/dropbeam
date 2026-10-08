# Feedback triage — 2026-09-27

Read-only pass over every **open** issue in `lman80/dropbeam` (26), checked against branch `ios` @ `6d9566d` (build 0.52.11). That branch is 36 commits ahead of origin/main and origin/ios. Recently closed issues (#16–#34, #38, #48–#63, all closed 2026-09-27 11:10 UTC) were spot-checked too. Every cited commit exists and matches its claim, and none look wrongly closed (details at the bottom).

Status key: **FIXED-ON-IOS** (fix is on `ios` and ships with it), **PARTIAL**, **NOT-FIXED**, **NEEDS-OWNER-INPUT**, **OBSOLETE**, **TEST**.

## Summary table

| # | Title (short) | Platform | Status | Size | Group |
|---|---|---|---|---|---|
| 68 | Copy should be a visible hover action, not behind ⋯ | mac/win/linux desktop | NOT-FIXED | S | Fix now |
| 67 | Clicking a Mac notification doesn't open that chat | mac (and win/linux) desktop | NOT-FIXED | M | Fix now |
| 66 | Send To: feedback button disappears; person menu opens off to the side | iOS | NOT-FIXED (regression from d63426c) | S | Fix now + owner Q |
| 46 | Chat header avatar tiny / under Dynamic Island, everything small | iOS | NOT-FIXED | M | Fix now |
| 36 | Typing bubble doesn't auto-scroll (and back) | iOS | NOT-FIXED (handler exists but doesn't work) | S | Fix now |
| 37 | ⌘C a file in Finder → ⌘V in chat attaches it | mac/win/linux desktop | NOT-FIXED | S–M | Fix now |
| 47 | Rich link previews in chat (like iMessage) | desktop + iOS | NOT-FIXED | M–L | Needs owner |
| 35 | Send card: progress "radius" + long-title layout | mac desktop | PARTIAL (title fixed; ring not built) | S–M | Fix now + owner Q |
| 33 | Receive card: Dock progress when minimized; says "Tauri App" | mac + linux desktop | PARTIAL | S | Fix now |
| 30 | See Location uploads in Locations; stuck "Connecting" at 15% | desktop | PARTIAL | M | Fix now |
| 31 | One account on phone + computer, see other device's progress | all | PARTIAL (accounts shipped; cross-device progress not) | M–L | Needs owner |
| 12 | Show who added a file (Blip-style badge) | mac | PARTIAL (xattr + notification shipped; no badge/chip) | M (chip) / L (Finder ext) | Needs owner |
| 20 | Auto-detect a safe speed limit | desktop | NOT-FIXED (design only) | M–L | Needs owner |
| 45 | Transfer Server (store-and-forward + iOS push) | all | FIXED-ON-IOS | — | Done |
| 44 | Chat shows online/offline + offline note | iOS (+desktop parity) | FIXED-ON-IOS | — | Done |
| 43 | Invite: QR picture share, paste anything, scan from photo, P2P explainer | iOS (+desktop parsing) | FIXED-ON-IOS | — | Done |
| 42 | Onboarding (name, photo, devices, friend, notifications) | iOS | FIXED-ON-IOS | — | Done |
| 41 | Swiping between multiple photos broken | iOS | FIXED-ON-IOS | — | Done |
| 40 | Feedback button missing in chat | iOS | FIXED-ON-IOS | — | Done |
| 39 | iPhone avatars lack colours/photos | iOS | FIXED-ON-IOS | — | Done |
| 65 | Crash SIGSEGV (0.52.8) | iOS | FIXED-ON-IOS (likely) | — | Done, watch |
| 64 | Crash SIGTRAP (0.52.8; dups #60/#61 were tagged 0.52.9) | iOS | FIXED-ON-IOS (likely) | — | Done, watch |
| 62 | Crash `__NSSingleObjectArrayI inputViewController` (0.52.8) | iOS | FIXED-ON-IOS | — | Done |
| 58 | Crash `__NSSingleObjectArrayI _controlTouchBegan` (0.52.8) | iOS | FIXED-ON-IOS | — | Done |
| 57 | Crash `WebActionDisablingCALayerDelegate applicationWillResignActive` (0.52.8) | iOS | FIXED-ON-IOS | — | Done |
| 53 | Crash SIGABRT ×5 on iPhone14,2 (0.52.7, LA time zone) | iOS | FIXED-ON-IOS (likely) | — | Done, watch |

**Counts:** FIXED-ON-IOS 13 (7 feature/bug + 6 crash) · PARTIAL 5 · NOT-FIXED 8. Of these, 5 need owner input: #47, #31, #12 and #20 are blocked on a decision, and #66 and #35 also carry a design question but can be fixed now.

---

## A. Fix now (ranked by user impact)

### #66 — iOS Send To: feedback button vanishes; person menu opens in the wrong place — NOT-FIXED, S
- **Cause 1 (button):** `SuperFeedback.swift` `refreshHostPresentation()` (~l.486) sets `hostPresenting` whenever any host window has a `presentedViewController`. `showsButton` then hides the trigger. This was deliberate in **d63426c** ("feedback button under sheets"). An iOS 26 `confirmationDialog` counts as a presentation, so tapping a person hides the button. The owner has now said it should *never* disappear.
- **Cause 2 (menu position):** `UI/SendView.swift:74` attaches `.confirmationDialog(sendingTo…)` to the **whole screen**. On iOS 26 it renders as a popover anchored to that view, not to the tapped avatar.
- **Fix:** In the Send To strip (and the same pattern in `FriendsView.swift:100` / `:251`), make each avatar a `Menu { Photos & Videos / Files / Folder } label: { avatar }`, or attach the dialog to the avatar button itself. The menu then anchors under the circle. For the button, stop hiding it for popovers and alerts: only step aside for full-height sheets, or keep it visible and let the sheet's detent push it up. Once the policy is chosen this is S.
- **Verify:** simulator is enough (the XCUITest harness in `tests/ios-uitests`).

### #67 — Clicking a desktop notification doesn't open the chat — NOT-FIXED, M
- iOS already works: `src/lib/chatNotifications.ts` is gated `IS_IOS`, and a tap goes to `openChat`.
- Desktop uses the vendored `tauri-plugin-notification/src/desktop.rs` `show()`. That calls `notify_rust` fire-and-forget, so no click is ever observed. `iroh_net.rs` ~l.5892 (`notify_chat`) only attaches `chatPeerId` extras on iOS.
- **Fix:** Add a desktop "show with click callback" path. On macOS, use `mac_notification_sys` with `wait_for_click(true)` on a blocking thread. On Linux, use notify_rust `handle.wait_for_action`. On Windows, use the winrt toast `on_activated`. When a click arrives, emit `chat-notification-open {peerId}`, show, unminimize and focus `main`, and call `store.openChat(peerId)`. Fallback: remember the last notified peer and open it on `RunEvent::Reopen` or app activation within about 30 s.
- **Risk:** `mac-notification-sys` uses the deprecated NSUserNotification API, which can't be tested well in dev because it's attributed to Terminal.
- **Verify:** one Mac plus any friend device to send a message (the Lab box works).

### #46 — iOS chat header: tiny avatar, crowding the Dynamic Island — NOT-FIXED, M
- The screenshot is from 0.52.5 and shows the **current** design: `ConversationView.swift` `header` (~l.230), a 36-pt `ContactAvatar` stacked over a glass name capsule in `ToolbarItem(.principal)`. The principal slot is capped at nav-bar height, so the avatar sits high and small.
- **Fix:** Build a Messages-style tall header. Hide the nav bar's own title area (or use `.toolbar(.hidden, for: .navigationBar)`) and put a custom glass header in `.safeAreaInset(edge: .top)`: back button, a 52–56-pt avatar under the island, then name + presence. Bump chat text to `.body` and the timestamps too if they look small. Check the tap-to-detail and search-bar placement still work.
- **Verify:** simulator plus one real iPhone (Dynamic Island).

### #36 — Typing bubble doesn't auto-scroll — NOT-FIXED, S
- A handler already existed before the report: `ConversationView.swift:136` `.onChange(of: bridge.chatTyping[friendID]) { if nearBottom { scrollDown(proxy) } }`, since da7e7fe (9/21). It fires in the same transaction as the animated insert, so `scrollTo("thread-bottom")` runs before the bubble is laid out, and nothing happens when the bubble goes away.
- **Fix:** Defer the scroll one runloop (`DispatchQueue.main.async` / `Task { await Task.yield() }`) and scroll again after the spring. Or better, use `.defaultScrollAnchor(.bottom, for: .sizeChanges)` (iOS 18+) so content-size changes stay pinned while `nearBottom`. Also scroll when typing turns off.
- **Verify:** simulator with the mock typing toggle.

### #68 — Copy should be one click — NOT-FIXED, S (desktop)
- `src/views/ChatView.tsx` ~l.1584: the hover bar (`.chat-actions`) has React / Reply / More. Copy is only inside More (`menuItems`, ~l.1484).
- **Fix:** When `copyable`, add `<IconButton label="Copy"><Copy/></IconButton>` between Reply and More, reusing `doCopy`, with the same toast. Optional: ⌘C copies the hovered bubble when nothing is selected. Frontend only; the dev preview is enough to verify.

### #37 — ⌘C a Finder file → ⌘V in chat attaches it — NOT-FIXED, S–M (desktop)
- `ChatView.tsx` `onPaste` (~l.735) only handles images; for anything else it defers to the webview. When a Finder file is copied, WKWebView exposes the **file's icon image** or its name as text, not the file.
- **Fix:** New command `clipboard_file_paths()`:
  - macOS: `NSPasteboard.generalPasteboard` `readObjectsForClasses:[NSURL]` with `NSPasteboardURLReadingFileURLsOnlyKey`. `tray_drag.rs` already has the objc2 pasteboard plumbing.
  - Windows: `CF_HDROP`.
  - Linux: `text/uri-list` (via GTK/wl-paste).
- In `onPaste`, ask for file paths **first**. If there are any, `preventDefault` and `stageChatFiles(paths)` (folders included). Also consider the Send & Receive drop zone.
- **Verify:** one Mac. Windows/Linux can follow.

### #33 — Receive card: Dock progress when minimized; "Tauri App" label — PARTIAL, S
- Done: `src/lib/taskbar.ts` puts progress on the Dock icon, but only when the **main** window is minimized (`App.tsx:112`).
- Missing 1: The **receive card** window (the one #33 minimizes) never drives the Dock bar. `ReceiveCard.tsx` doesn't call `setTaskbarProgress`.
- Missing 2: The `receive` window in `src-tauri/tauri.conf.json` (~l.62) has **no `"title"`**, so Tauri's default "Tauri App" shows in the Dock and Linux taskbar. This is the "tarryapp" in the report.
- **Fix:** Add `"title": "DropBeam"` to the receive window. From `ReceiveCard`, call the taskbar helper with the card window's own minimized state: on macOS the Dock tile is app-wide, so it can be set from any window.
- **Verify:** one Mac plus a sender. The "whole radius is the progress" part is #35.

### #35 — Send card progress "radius" + long title — PARTIAL, S–M
- Done: long names are middle-truncated on one line and the card is sized to fit (78a0735, 763e4dd; also closed #29).
- Not done: the owner has asked several times for the progress to be shown as a thick ring. The card (`src/windows/ReceiveCard.tsx`) still uses a linear `ProgressBar` in `.rc-progress`.
- **Fix:** An SVG rounded-rect stroke that follows the card's corner radius (`stroke-dasharray`/`dashoffset` = percent). Or a thick ring around `.rc-avatar`, like Blip. See owner question 1.
- **Verify:** one Mac.

### #30 — Location uploads not visible in Locations; stuck "Connecting…" — PARTIAL, M
- The inline "Connecting…" pill in the screenshot is gone. Since fed9782, path info sits behind an ⓘ (`ConnInfo`), and `ConnInspector` hides `connecting`. The ⓘ can still say "Connecting" while bytes flow, because `humanize.ts` `pathKind` trusts a stale `connDetail.path === 'connecting'`.
- Still missing: an **outgoing** upload to a friend's Location only toasts "follow it in Send & Receive" (`FileBrowser.tsx:64`). `LocationsView` shows inbound uploads only (`incomingByLocation`).
- **Fix:**
  1. In `pathKind`, when the transfer is `transferring` with bytes advancing, fall back to the last known path or locality instead of "connecting". Better still, fix the engine to refresh `connDetail` after a stream reconnect.
  2. Add an "Uploading here" row with progress, pause and cancel to the Location/FileBrowser view, filtered by transfer target location.
- **Verify:** Mac → Linux box NAS Location (two devices).

---

## B. Needs owner input (recommended answers in **bold**)

1. **#35 / #33 — which "radius"?** A ring around the whole card's rounded border, or a thick ring around the avatar/file art (Blip)? **Recommend the card-border ring:** it's what "the entire radius was the progress bar" describes, and it also reads at a glance when the card is small. Keep the % and speed text.
2. **#66 — feedback button policy.** You said it should never disappear, but d63426c hides it under sheets so it doesn't float over sheet content. **Recommend: never hide it for menus, popovers or alerts; for full sheets, keep it visible but lift it above the sheet's bottom controls** (the same "bottom obstruction" measurement the chat uses).
3. **#47 — link previews and privacy.** Fetching a page preview reveals your IP to that site. **Recommend the iMessage model:** only the **sender's** device fetches (iOS `LPMetadataProvider`; desktop a bounded Rust fetch of `og:title`/`og:image`, max ~1 MB, 5 s timeout), and a small thumbnail plus title travel inside the chat message, so the receiver never contacts the site. Add a Settings toggle "Show link previews" (default on). About M–L across both platforms.
4. **#31 — is the shipped account sync enough?** Own-device accounts are live on `ios` (`account.rs`: shared roster, friends, chats, avatars, Devices list, and the fan-out send to all a friend's devices). What's missing is "see the progress of what my other device is sending". **Recommend: yes, build a read-only "On your other devices" section in Send**, mirrored from a compact transfer digest over the existing account-sync connection. No cross-device cancel in v1. M–L, needs two own devices.
5. **#12 — Finder badge now that you have an Apple Developer account?** The xattr `com.dropbeam.from` and the "from X" notification already ship, and a Finder Sync extension is written (`macos/finder-sync/`) but not embedded in CI. **Recommend: first an in-app "from X" chip** in the Locations FileBrowser and History Recents (read the xattr in batches; M). Then embed the Finder Sync extension as its own signed/notarized release step (L). Finder can only show a generic "from a friend" badge, not the avatar.
6. **#20 — still wanted?** Only a static hint exists ("Start at 100 Mbps…", `SettingsView.tsx:245/471`), and local-network transfers ignore the limit anyway. **Recommend: park it (low impact)** unless the router crashes again. If wanted: a "Find my safe speed" ramp on a direct path that recommends 70% of the last healthy step and never applies anything automatically (design in `docs/feedback-triage.md`).

---

## C. Done on `ios` — close once the branch ships

| # | Evidence |
|---|---|
| 39 | **ba02a6f**: iOS uses desktop's palette and hash (`avatar.ts`) plus initials. Stored photo paths are re-resolved into the current app container (the real reason photos never showed). A friend-hello reply carries the picture. |
| 40 | **7490e0f**: the button is parked above the composer inside chats (`SuperFeedback.setBottomObstruction`, `ConversationView.swift` ~l.126). Screenshots skip keyboard windows (black-capture fix). |
| 41 | **61c2054**: viewer opens on the tapped photo, clean paging, "n of N", zoom that pans instead of paging. Covered by the XCUITest harness (ad1d571). |
| 42 | **56e9b6c**: `UI/Onboarding.swift`, steps welcome → name → photo → devices → friend → notifications, animated P2P intro, every step skippable. Existing users don't see it. **1e66d48** hides the feedback button during setup. |
| 43 | **56e9b6c** + **991959b** + **a618186**: the invite card is shared as a picture (photo, name, QR, how-to) plus a paste-able message. Add Friend accepts a pasted message, code or link, Scan from Photo and the camera. The P2P "keep DropBeam open" explainer is in `Invite.swift:31/206/332`. Tappable link via `lman80.github.io/dropbeam-invite` (live, HTTP 200). |
| 44 | **41531c9**: header shows Online / Connecting… / Last seen / Offline, with an 8 s check and a 30 s re-check. Offline note above the composer and "Waiting to send". Desktop parity too. Visible in the #46 screenshot ("Online"). |
| 45 | **9636ed5 … 6d9566d**: Transfer Server phases 1–3 (`src-tauri/src/mailbox/*`, `TransferServerSettings.tsx`, `ServerExplainer.tsx`, `UI/TransferServer.swift`), APNs push (97eb155, a8b2afe), headless mode. Status in `docs/TRANSFER-SERVER-PLAN.md`. Worth one real-world two-device confirmation (phone asleep → push → message lands) before closing. |
| 57, 58, 62 | **019ce1e** (build 0.52.9): push registration no longer runs `UIApplication.delegate = nil; = delegate`, which freed the launch delegate. The "unrecognized selector sent to `__NSSingleObjectArrayI` / `WebActionDisablingCALayerDelegate`" messages are that freed-delegate pattern: a released object's address reused. |
| 64, 65, 53 | Bare signals (no stack), all from 0.52.7/0.52.8 builds that contain the bad delegate re-assign (added in 0e65e7f, before 0.52.7). **No crash report since 2026-09-26 14:07 UTC**, including a 9-hour session on 0.52.10 (#66, uptime 32 059 s). Caveat: dups #60/#61 (SIGTRAP) are tagged build 0.52.9, so keep #64 open until about a week crash-free on ≥0.52.10. #53's device (iPhone14,2, LA) must update past 0.52.9. |

---

## Recently closed issues (sanity check)

All 29 closures from 2026-09-27 11:10 UTC cite real commits that match their claims (56a2d2c, 763e4dd, 78a0735, 2042c82, 4547244, 5a48b12, 3f791fe, 010c4a4, 57e8693, 5ed1d88, b3ff838). Crash duplicates were grouped by signature.
- **#38** (closed as dup of #53) is actually from the **Simulator** on 0.52.2, before any push code existed. That makes it a different cause, and it's dev noise. Closing it is still right; the "duplicate" label is just inaccurate.
- **#29** (long name) is closed, and #35's remaining ask is the progress ring, not the title.
- Crash reports only say "App crashed in a previous session: Fatal signal X", with no stack for signal crashes. A future improvement is to attach the last N lines of the panic/engine log, or the MetricKit `MXCrashDiagnostic` payload, to recovered-crash reports.
