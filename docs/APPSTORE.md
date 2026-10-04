# DropBeam for iPhone — App Store readiness

Checklist for shipping the native iOS app (SwiftUI shell in
`src-tauri/plugins/native-ui/ios/…`, project in `src-tauri/gen/apple`) to the App
Store. Status: ✅ done in the repo · ⚠️ done but needs a decision/check · 👤 owner action
in App Store Connect (ASC) or outside the repo.

App: **DropBeam** · bundle id `com.ashtonmiller.dropbeam` · team `R2RDA8476R` ·
version from `tauri.conf.json` (marketing 0.52.2, iOS build `bundle.iOS.bundleVersion` 0.53.0) · iPhone only (`TARGETED_DEVICE_FAMILY 1`),
portrait · iOS 17+ (Liquid Glass on iOS 26+, materials before).

---

## 1. Binary & project configuration

| # | Item | Status | Notes |
|---|---|---|---|
| 1.1 | Usage strings are accurate and friendly | ✅ | `NSCameraUsageDescription` (QR scanning only), `NSPhotoLibraryAddUsageDescription` (Save Image from the share sheet), `NSLocalNetworkUsageDescription` + `NSBonjourServices = _irohv1._udp, _dropbeam._udp` (iroh mDNS + DropBeam's own system-Bonjour discovery, see §1.14; without both keys LAN peers never appear). Photos are picked with PHPicker → no read-library permission, no `NSPhotoLibraryUsageDescription` needed. |
| 1.2 | No macOS-only keys in the iPhone Info.plist | ✅ | `src-tauri/Info.plist` is now shared keys only; `LSUIElement`, the folder-access prompts and `NSServices` moved to `src-tauri/Info.macos.plist` (`bundle.macOS.infoPlist`). tauri-cli 2.11 merges `Info.plist` **then** `bundle.macOS.infoPlist` for macOS (verified in `crates/tauri-cli/src/interface/rust.rs`), so the desktop bundle keeps every key. `gen/apple/app_iOS/Info.plist` cleaned to match. |
| 1.3 | Export compliance key | ✅⚠️ | `ITSAppUsesNonExemptEncryption = false` in the app's Info.plist (project.yml → app_iOS/Info.plist; verified in the built bundle). This matches the answer already given per build in App Store Connect (`usesNonExemptEncryption=false`): DropBeam only uses standard, published encryption (TLS 1.3/QUIC via rustls, Ed25519/X25519, ChaCha20-Poly1305) for authentication and end-to-end protection of the user's own data, which Apple treats as exempt; the key just stops ASC asking on every upload. ⚠️ If legal review ever concludes otherwise, delete the key and answer per build (`true` without an `ITSEncryptionExportComplianceCode` is rejected at upload, ITMS-90592). France: see Apple's export-compliance guidance. |
| 1.4 | Privacy manifests | ✅ | App (`gen/apple/app_iOS/PrivacyInfo.xcprivacy`) required-reason APIs: File timestamp `C617.1` `3B52.1` `DDA9.1`; Disk space `E174.1` `85F4.1` (statvfs in `locations.rs`); UserDefaults `CA92.1`; System boot time `35F9.1`. Tracking: none. Collected data: see §3. The share extension (`DropBeamShare/PrivacyInfo.xcprivacy`: File timestamp `C617.1` for the copies it makes) and the notification extension (`DropBeamNotify/PrivacyInfo.xcprivacy`: nothing) ship their own. |
| 1.5 | App icon set complete, opaque | ✅ | All iPhone sizes + 1024 marketing icon present; every PNG re-encoded **without an alpha channel** (ITMS-90717). If icons are ever regenerated with `tauri icon`, re-flatten them (the script used: draw into an `noneSkipLast` CGContext and re-save). Optional later: iOS 18 dark / tinted icon variants. |
| 1.6 | Launch screen | ✅ | `LaunchScreen.storyboard`, plain `systemGroupedBackground` = the first screen's background (no logo flash, per HIG). |
| 1.7 | Devices & orientation | ✅ | iPhone only, portrait only (`project.yml` + Info.plist). iPad keys removed. |
| 1.8 | Background modes | ✅ | No `UIBackgroundModes`. A transfer that is moving when the user leaves the app continues under `beginBackgroundTask` (no mode needed; ended as soon as nothing moves or iOS expires it). On iOS 26+, a send the user starts that is ≥ 20 MB also submits a `BGContinuedProcessingTask` (identifiers `com.ashtonmiller.dropbeam.transfer.*` in `BGTaskSchedulerPermittedIdentifiers`; system progress UI; registered dynamically right before submit). ⚠️ Confirm on a device that submission isn't refused without a background mode; if it is, the code logs and falls back to the background task — never add `audio`/`fetch`/`processing` to keep transfers alive (2.5.4). |
| 1.9 | No private APIs | ✅ | UI is public SwiftUI/UIKit/VisionKit/AVFoundation/PhotosUI. `shareddocuments://` (Open in Files) is a documented URL scheme. Rust side uses public ObjC classes via `objc2` (PHPicker, UIDocumentPicker). |
| 1.10 | Files app visibility & storage | ✅ | `UIFileSharingEnabled` + `LSSupportsOpeningDocumentsInPlace` → received files (and Shared Folder copies, `Shared Folders/<name>`) in Files › On My iPhone › DropBeam. Copies of what the user picks/pastes/shares to send live in Application Support/`dropbeam-picked` instead: not visible in Files, excluded from backup, swept once sent (Send-tab copies 1 day after finishing, chat attachments 30 days, 2 GB cap, never while a send is unfinished or queued). |
| 1.11 | Entitlements | ✅ | App: `aps-environment` (Transfer Server push; `scripts/enable-push.sh`, docs/PUSH-SETUP.md), `com.apple.security.application-groups = group.com.ashtonmiller.dropbeam` (share + notification extensions), `keychain-access-groups` = the app's own group + `superfeedback.shared` (one anonymous feedback voter id across the owner's apps). DropBeamShare / DropBeamNotify: the same App Group only. No iCloud, no multicast (see 1.14). |
| 1.12 | Version/build numbers | 👤 | CFBundleVersion must increase on every upload: bump `bundle.iOS.bundleVersion` in `tauri.conf.json` (now 0.53.0); the extensions copy the app's versions at build time. |
| 1.13 | Minimum functionality 4.2 / 2.1 completeness | ✅ | Native SwiftUI UI end-to-end (the hidden WebView is only a data bridge, never shown or interactive). |
| 1.14 | LAN discovery / multicast entitlement | ✅ optional 👤 | iroh's own mDNS (swarm-discovery) sends raw multicast, which iOS only allows with `com.apple.developer.networking.multicast` (Apple grants it on request: developer.apple.com/contact/request/networking-multicast). Not needed to ship: the app advertises and browses `_dropbeam._udp` through the system Bonjour (NWListener/NWBrowser, `LanDiscovery.swift`) and hands found devices' LAN addresses to the engine (`lan_peer_found`). iPhone ↔ iPhone is found that way; desktop builds still use swarm-discovery (desktop advertising `_dropbeam._udp` is a follow-up). Requesting the entitlement later would let iOS also join iroh's own discovery. |
| 1.15 | Share extension opens the app | ⚠️ | Share extensions have no supported API to open their containing app (`extensionContext.open` is for Today/iMessage). DropBeamShare tries `openURL` via the responder chain for our own `dropbeam://share` scheme; if iOS refuses, the job waits in the App Group, a "Ready to send — tap to finish" notification is shown, and the app sends it as soon as its engine is up (first settings snapshot) or on the next foreground. If Review objects, drop the responder-chain call: the notification + pickup path already covers it. |

## 2. In-app requirements

| # | Item | Status | Notes |
|---|---|---|---|
| 2.1 | Privacy policy link in the app | ✅ | Settings → Privacy & Support → Privacy & Your Data → Privacy Policy, and Diagnostics → Privacy Policy. URL: https://github.com/lman80/dropbeam/blob/main/PRIVACY.md |
| 2.2 | Support link | ✅ | Settings → Help & Support → https://github.com/lman80/dropbeam/issues (also the ASC Support URL), plus Send Feedback. |
| 2.3 | Data deletion (5.1.1(v)) | ✅ | There is no server account (identity is a key on the device), but Settings → Privacy & Your Data → **Erase All Data** deletes everything DropBeam keeps on the iPhone: withdraws the push token from every Transfer Server, removes the iPhone from the user's linked devices (the others keep their data), unregisters push, wipes identity/friends/chats/history/settings/received files in the DropBeam folder/picked copies/caches/App Group/preferences, then closes; the next launch is a fresh install. Files saved to another folder the user chose and items saved to Photos stay (said in the confirmation). |
| 2.4 | User-generated content (1.2) | ✅ | See **§2a Moderation** below: **Block** (friend page, friend-list long-press/swipe, conversation ⋯ menu) and **Report** (friend page, conversation ⋯ menu, long-press on any received message or file) are built in, plus Settings → **Blocked** (unblock) and **Report a Problem**. Chat and files only flow between people who exchanged codes. GIFs use Giphy `rating=pg-13` and are off until the user adds their own key. |
| 2.5 | Diagnostics are opt-out and honest | ✅⚠️ | Settings → Diagnostics → *Share Diagnostics* (default on, redacted daily digest + crash reports). Crash reports now follow that switch too (`SuperFeedback.setCrashReportingEnabled`). ⚠️ see §3. |
| 2.6 | Floating feedback button | ✅ | On by default only in TestFlight/debug/simulator builds (App Store installs start with it off; Settings → Feedback Button turns it on). Docks half into the screen edge, keeps clear of nav/tab bars, hides with the keyboard and inside a conversation, never over row controls. |
| 2.7 | Permissions asked in context | ✅ | Camera: only when a scanner opens (paste fallback + "Open Settings" if denied). Local Network: iOS prompts on first discovery. Notifications: requested by the notification plugin at first launch. Paste uses `PasteButton` everywhere, including Paste Image in a chat (no "Allow Paste" prompt). |
| 2.8 | Accessibility | ✅ | Every icon-only button has a label; Dynamic Type (verified at XXXL; rows wrap instead of truncating); VoiceOver rows combine; Reduce Motion stops the background drift; light + dark. |

## 2a. Moderation (guideline 1.2) — how it works

DropBeam has no server and no public content: chat and files only flow between two people who exchanged codes. Moderation tools:

- **Block** — confirmation dialog explains the effect. Removes the person on all the user's linked devices (the blocked list syncs over the account sync, `account::Meta.blocked`; an unblock syncs too). Enforced **in the engine** (`src-tauri/src/block.rs`, gate in `iroh_net::serve_stream_inner`): the blocked person's hello can't re-add them, and their chat, typing/read signals, file pushes (+ stat/verify), folder invites and Location requests are answered exactly as a stranger's — they aren't told. Every device of theirs we know is blocked (grouped by their verified account), and a new device proving the same account is blocked on its first hello. Unblock: Settings → **Blocked**.
- **Report** — on a person or on a single message/file. The user picks a reason (spam, harassment, hate/threats, sexual content, illegal/dangerous, impersonation/scam, other), chooses whether to include the message text (files are never attached — only the file name, if they choose), adds optional details and can block at the same time. It opens a pre-filled email to the developer (`REPORT_EMAIL` in `src/lib/report.ts` = imamiller64@gmail.com, the App Store contact) in the user's own mail app. If no mail app is set up, iOS offers to copy the report.
- **Response commitment:** reports are reviewed **within 24 hours**. Because content never passes through a DropBeam server, the developer can't delete it remotely; the response is to contact the reporter, and ask them to block (if they haven't); repeat or illegal abuse is referred to the appropriate authorities. The app text promises the 24-hour response.
- **Contact:** Settings → **Report a Problem** (iOS) / *Privacy & safety* → **Email us** (desktop) opens an email to the same address.

## 3. App Privacy (ASC → App Privacy) — answers that match the manifest

Data **not** collected: files, photos you send, messages, contacts, location, identifiers for tracking. Transfers and chats are end-to-end encrypted device-to-device; relays only forward ciphertext.

| Data type | Collected | Linked to user | Tracking | Purpose | Source |
|---|---|---|---|---|---|
| Other Diagnostic Data | Yes | No | No | App Functionality | Share Diagnostics digest (opt-out) |
| Performance Data | Yes | No | No | App Functionality | same digest (speeds, relay vs direct) |
| Device ID | Yes | No | No | App Functionality | random per-install `diag-id` in the digest |
| Crash Data | Yes | No | No | App Functionality | crash reports incl. MetricKit crash stacks (follow Share Diagnostics) |
| Customer Support | Yes | No | No | App Functionality | Send Feedback message + device info; a Report email the user sends (reason, their notes, optionally the reported message text, app version) |
| Photos or Videos | Yes | No | No | App Functionality | optional screenshot/images attached to feedback |

The digest no longer carries the display name (`telemetry.rs` sends only the random per-install `deviceId`), so nothing collected is linked to the user and **Name** is not declared; the manifest matches this table.

Optional: if you want to be conservative about the Giphy integration (off by default, user supplies the key, queries go from the phone to Giphy), declare **Search History — not linked — App Functionality**.

## 4. Age rating (ASC questionnaire)

| Question | Answer |
|---|---|
| Violence / sexual content / profanity / horror / drugs / gambling / contests | None |
| Medical or treatment info | No |
| Unrestricted web access | No (links open Safari; no in-app browser) |
| User-generated content | Yes — private chat and file sharing between people who exchanged codes |
| Messaging and chat | Yes |
| Advertising | No |
| Parental controls / age assurance | No |

Expected result: **13+** under the 2025 age-rating system (messaging + UGC). 👤 Confirm in ASC.

## 5. App Review notes (paste into ASC → App Review Information)

> DropBeam sends photos, files and folders directly between devices (peer-to-peer, end-to-end encrypted) and lets friends chat. There is no account or sign-in — the app creates a private key on first launch and only asks for a display name.
>
> **Testing with one device:** tap **Send → Photos**, pick a photo, then **Quick Send with a Code**. DropBeam shows a QR code and text code. On any second device with DropBeam (another iPhone/iPad, or the free Mac/Windows/Linux app from https://github.com/lman80/dropbeam/releases) open **Send → Have a Code?** (or scan the QR) to receive it.
>
> **Demo friend:** we keep a Mac running DropBeam online during review. Add it with **Friends → + → Add Friend → Enter Code Instead** and paste: `<REVIEW FRIEND CODE>`. It accepts files automatically and you can message it from **Chats**. (Please don't send large files.)
>
> Local Network access is used to find devices on the same Wi-Fi so transfers go directly; the camera is used only to scan QR codes. A transfer keeps going briefly if you leave the app (a standard background task; on iOS 26 a large send shows the system's progress); there are no background modes. Chat is only possible between people who exchanged codes.
>
> **Blocking and reporting (1.2):** open a friend's page (Friends → the friend) or a conversation's **⋯** menu → **Block** / **Report…**; long-press any received message → **Report…**. Blocking removes the person on all the user's devices and the app refuses their messages, files and invites from then on (they aren't notified); Settings → **Blocked** lists and unblocks. A report opens a pre-filled email to us (reason, optional message text — never files) and can block at the same time. We review every report within 24 hours. Feedback/support: Settings → Report a Problem, Send Feedback, or https://github.com/lman80/dropbeam/issues.

👤 Owner: set up a Mac as the review friend (a separate DropBeam install/identity named e.g. "DropBeam Review", auto-accept on, online during review), paste its friend code (Settings → Profile → Copy on the Mac) into the note, and add a contact phone/email.

## 6. Store listing (draft)

- **Name** (≤30): `DropBeam` — fallback if taken: `DropBeam – P2P File Transfer` (28)
- **Subtitle** (≤30): `Send files straight to friends` (30) — alt: `Private, direct file sharing` (28)
- **Promotional text** (≤170): Big videos, whole folders, full-quality photos — beamed straight to friends and your own devices. No accounts. No cloud. End-to-end encrypted. (142)
- **Keywords** (≤100): `file transfer,send files,share,p2p,photos,video,large files,wifi,nas,encrypted,sync,folder,chat,qr` (98) — no competitor/Apple trademarks (e.g. never "AirDrop").
- **Primary category:** Utilities · **Secondary:** Productivity
- **Copyright:** © 2026 Ashton Miller
- **Support URL:** https://github.com/lman80/dropbeam/issues · **Marketing URL:** https://github.com/lman80/dropbeam · **Privacy Policy URL:** https://github.com/lman80/dropbeam/blob/main/PRIVACY.md

**Description**

> DropBeam sends anything to anyone — across the room or across the world — straight from your iPhone to theirs.
>
> SEND WITHOUT THE WAIT
> • Photos and videos at full quality, documents, even whole folders
> • Nearby devices connect over your Wi-Fi at full local speed; far away, DropBeam finds a direct path over the internet
> • Pause, resume and verify big transfers — every file is checked end to end
>
> FRIENDS, NOT FOLLOWERS
> • Add friends by scanning their QR code — no phone number, no email, no sign-up
> • Send to a friend by name, or share a one-time code with anyone
> • Chat with reactions, replies, photos and GIFs
>
> ALL YOUR DEVICES
> • Link your Mac, PC or another phone so your friends and chats follow you
> • Shared Folders stay in sync with friends while DropBeam is open
> • Browse and download from folders and drives friends share with you
>
> PRIVATE BY DESIGN
> • End-to-end encrypted, device to device — your files never sit on our servers
> • No account, no ads, no tracking
>
> DropBeam is free and works with the DropBeam apps for Mac, Windows and Linux.

**What's New** (first release)

> Welcome to DropBeam for iPhone: send photos, files and whole folders straight to friends and your own devices, chat, sync shared folders and browse friends' drives — all end-to-end encrypted, no account needed.

## 7. Screenshots

✅ 7 screenshots at **1320 × 2868** (6.9" display size; ASC scales them down for smaller iPhones), light mode, opaque PNG, demo identity "Jamie Rivera" and sample photos only, clean 9:41 status bar. Location (not committed): `~/DropBeam-wt/iosui-scratch/appstore/`

1. `01-send.png` — Send: Photos / Files / Folder + Have a Code
2. `02-quick-send-code.png` — Quick Send QR + code for 3 photos
3. `03-friends.png` — Friends: Locations, Shared Folders, My Code, friends list
4. `04-friend.png` — Friend detail: Send / Message / Check (Local · 14 ms)
5. `05-send-to.png` — Send To chooser (friends, devices, Quick Send)
6. `06-my-code.png` — My DropBeam code (QR) to add friends
7. `07-settings.png` — Settings

How they were made: captured from the iPhone 17 Pro simulator (1206 × 2622) and Lanczos-scaled to 1320 × 2868 — the aspect ratios match to 0.07 %, so nothing is stretched. (A dedicated iPhone 17 Pro Max simulator was created for native captures but couldn't be driven by the automation, so it was deleted.) ⚠️ `05-send-to.png` shows the half-docked feedback handle at the right edge; retake it with Settings → Feedback Button off if you want it perfect. For pixel-native 6.9" captures, run the app on an iPhone 17 Pro Max simulator with the Feedback Button off and `xcrun simctl status_bar <udid> override --time 9:41`.

👤 Upload them in ASC → iPhone 6.9" screenshots (the first 3 show in search results). Optional: add captions/device frames in a design tool.

## 8. Before submitting (owner)

1. 👤 Update the App Privacy answers to §3 (Name removed; diagnostics, performance and device id **not linked**).
2. 👤 Export compliance answers (§1.3) + France decision.
3. 👤 Review friend Mac + code in the review notes (§5).
4. 👤 Age rating questionnaire (§4), pricing (Free), availability.
5. 👤 Archive a **Release** build (`npx tauri ios build --export-method app-store-connect` with the team's signing), upload (Transporter / `xcrun altool` with the ASC API key), wait for processing, attach to the version, submit.
6. Smoke test the Release build on a real iPhone: onboarding, Quick Send between two devices, add friend by QR, chat, Local Network prompt, camera prompt, Open in Files.
