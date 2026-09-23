# DropBeam — feature parity across platforms

Source of truth: the code on branch `ios` as of v0.52.2 (Rust engine `src-tauri/src`, desktop React UI `src/`, native iOS SwiftUI shell `src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin`, CI `.github/workflows/release.yml`). The user-facing guide is [FEATURES.md](FEATURES.md); the screen list is [UI-INVENTORY.md](UI-INVENTORY.md).

**Legend:** ✅ works · ⚠️ partial (note says what's missing) · ❌ not available · N/A = doesn't apply to the platform.

**How each platform ships:** macOS universal `.dmg`, Windows `.exe`/`.msi`, Linux `.deb` + `.AppImage` — all from `release.yml` on a `v*` tag, with in-app updates. iOS is built separately (TestFlight; not in `release.yml`). The desktop UI is one React bundle for all three desktops; iOS uses the same Rust engine, and the SwiftUI screens reach it through a hidden WebView bridge (`src/lib/nativeBridge.ts` handlers ↔ `Bridge.swift`).

---

## 1. Feature × platform matrix

### Account, profile, devices
| Feature | macOS | Windows | Linux | iOS | Notes |
|---|---|---|---|---|---|
| First-run name prompt | ✅ | ✅ | ✅ | ✅ | |
| Profile picture (set/remove) | ✅ | ✅ | ✅ | ✅ | |
| Your friend code + QR | ✅ | ✅ | ✅ | ✅ | iOS: Settings → Profile (+ Share sheet) |
| Link devices into one account (add / join / show code / scan) | ✅ | ✅ | ✅ | ✅ | |
| Devices list, Sync now, remove device, leave account | ✅ | ✅ | ✅ | ✅ | desktop uses `window.confirm` dialogs |
| Own devices labelled "Your Mac" etc.; multi-device friend folds to one person | ✅ | ✅ | ✅ | ✅ | |

### Sending
| Feature | macOS | Windows | Linux | iOS | Notes |
|---|---|---|---|---|---|
| Drag & drop into window | ✅ | ✅ | ✅ | N/A | |
| Pick files / photos | ✅ | ✅ | ✅ | ✅ | iOS: PHPicker + document picker, "Preparing…" overlay with Cancel |
| Send a whole folder | ✅ | ✅ | ✅ | ❌ | iOS Files picker is `.item` only (`NativeFolderPicker.pickFiles`); folder upload exists only for Locations |
| Send-to chooser (My Devices / Friends / Quick Send) | ✅ | ✅ | ✅ | ✅ | |
| Send to friend by name | ✅ | ✅ | ✅ | ✅ | |
| Quick Send code + QR | ✅ | ✅ | ✅ | ✅ | |
| Menu-bar / tray quick menu | ✅ | ✅ | ❌ | N/A | Linux: Tauri tray click events aren't delivered, so `toggle_popover` never fires; only the Open/Quit menu shows |
| Drag onto tray icon → drop on a friend | ✅ | ❌ | ❌ | N/A | `tray_drag.rs` is macOS-only |
| OS "send with" integration | ✅ | ✅ | ❌ | ❌ | macOS Services "Share with DropBeam" (`mac_service.rs`); Windows right-click (`windows/hooks.nsh`); Linux none; iOS has no share extension |
| Scripted Location uploads | ⚠️ | ✅ | ✅ | ❌ | macOS: `upload-queue.json` only (no second-instance arg forwarding); Win/Linux also `--location-upload` |

### Receiving
| Feature | macOS | Windows | Linux | iOS | Notes |
|---|---|---|---|---|---|
| Auto-accept per friend | ✅ | ✅ | ✅ | ✅ | |
| Manual Accept / Decline | ✅ | ✅ | ✅ | ✅ | |
| "Save to…" per delivery | ✅ | ✅ | ✅ | N/A | sandbox: always Documents |
| Default download folder setting | ✅ | ✅ | ✅ | N/A | iOS fixed: Files → DropBeam |
| Receive with a code (paste) | ✅ | ✅ | ✅ | ⚠️ | iOS field accepts only Quick Send codes (`receiveWithCode` → `store.receiveCode`); desktop routes every code kind (`openCode`) |
| Scan QR (camera) | ✅ | ✅ | ⚠️ | ✅ | Linux: depends on WebKitGTK camera support (unverified); image/paste fallback always works |
| Scan QR from screenshot / pasted image | ✅ | ✅ | ✅ | ❌ | iOS: camera or paste text only |
| Show in folder / Share after receive | ✅ | ✅ | ✅ | ✅ | iOS: Share sheet |

### Transfers
| Feature | macOS | Windows | Linux | iOS | Notes |
|---|---|---|---|---|---|
| Progress, speed, ETA | ✅ | ✅ | ✅ | ⚠️ | iOS: no live/average toggles, no completion summary line |
| Path badge (Local/Direct/Relay, RTT, upgrading) on transfer cards | ✅ | ✅ | ✅ | ❌ | iOS `TransferCard` doesn't render `locality`/`connDetail` |
| Pause / Resume sends | ✅ | ✅ | ✅ | ⚠️ | iOS: "Resume" only appears if a transfer is already paused; no pause button |
| Cancel | ✅ | ✅ | ✅ | ✅ | |
| Retry failed send | ✅ | ✅ | ✅ | ✅ | iOS also retries a failed Quick Send receive |
| Dismiss finished card | ✅ | ✅ | ✅ | ❌ | finished/failed cards stay in the iOS Send tab |
| Integrity label + per-file SHA-256 details | ✅ | ✅ | ✅ | ❌ | engine checks run on iOS; UI doesn't show them (Send tab or History) |
| Verify copy (full re-hash on peer) | ✅ | ✅ | ✅ | ❌ | |
| Wait-for-direct park + "Send over relay anyway" | ✅ | ✅ | ✅ | ⚠️ | iOS has the setting but no escape button (`api.forceRelay` never reachable) |
| Taskbar/Dock progress | ✅ | ✅ | ✅ | N/A | |

### Friends
| Feature | macOS | Windows | Linux | iOS | Notes |
|---|---|---|---|---|---|
| Add by code / QR | ✅ | ✅ | ✅ | ✅ | |
| Rename / remove | ✅ | ✅ | ✅ | ✅ | |
| Auto-accept switch | ✅ | ✅ | ✅ | ✅ | |
| Presence + Check | ✅ | ✅ | ✅ | ✅ | |
| Connection inspector | ✅ | ✅ | ✅ | ⚠️ | iOS: one-line text on Check, no live "upgrading" or re-probe |
| Re-show one-time invite (`friend_invite`) | ✅ | ✅ | ✅ | ❌ | low value — permanent code covers it |
| Friend search | ❌ | ❌ | ❌ | ✅ | desktop Friends page has no search (popover does) |

### Chat
| Feature | macOS | Windows | Linux | iOS | Notes |
|---|---|---|---|---|---|
| Messages, offline outbox | ✅ | ✅ | ✅ | ✅ | |
| Reactions | ✅ | ✅ | ✅ | ✅ | different emoji sets: desktop 7 quick; iOS 6 tapbacks + 24 more |
| Reply / edit / unsend / copy | ✅ | ✅ | ✅ | ✅ | |
| Typing + read receipts | ✅ | ✅ | ✅ | ✅ | |
| Attach files/photos (staged) | ✅ | ✅ | ✅ | ✅ | |
| Drag files into conversation | ✅ | ✅ | ✅ | N/A | |
| Paste image | ✅ | ✅ | ✅ | ❌ | Linux via native clipboard fallback |
| GIFs (Giphy key) | ✅ | ✅ | ✅ | ✅ | |
| Emoji picker | ✅ | ✅ | ✅ | N/A | iOS keyboard |
| Search in conversation | ✅ | ✅ | ✅ | ✅ | |
| Search chat list | ❌ | ❌ | ❌ | ✅ | |
| Pin conversations | ❌ | ❌ | ❌ | ✅ | iOS-only (`@AppStorage("dropbeam.chat.pinned")`, per device) |
| Mark read from list | ❌ | ❌ | ❌ | ✅ | desktop marks read on open |
| New-message picker | ❌ | ❌ | ❌ | ✅ | desktop: start from a friend card |
| Shared-folder button + folder activity rows | ✅ | ✅ | ✅ | ❌ | activity rows live in desktop localStorage only |
| Notifications | ✅ | ✅ | ✅ | ⚠️ | iOS only while the app runs; tap opens chat on iOS |
| Unread badge on app icon | ✅ | ❌ | ❌ | ❌ | `set_unread_badge` is macOS-only; iOS shows the tab badge only |

### Shared Folders
| Feature | macOS | Windows | Linux | iOS | Notes |
|---|---|---|---|---|---|
| Create folder (modes, invite friends) | ✅ | ✅ | ✅ | ❌ | |
| Accept invite (prompt / paste / scan) | ✅ | ✅ | ✅ | ⚠️ | iOS can join, importing a copy into Documents/Imported Folders — then nothing shows it (see §3) |
| Folder list, status, completion summary | ✅ | ✅ | ✅ | ❌ | |
| Mirror / two-way / one-way / delete-after-delivery | ✅ | ✅ | ✅ | ❌ | iOS "Trash" delete mode degrades to plain delete (`sync.rs` `delete_local`) |
| Roles (editor/viewer), add person, remove member | ✅ | ✅ | ✅ | ❌ | |
| Pause / resume sync | ✅ | ✅ | ✅ | ❌ | |
| Verify folder | ✅ | ✅ | ✅ | ❌ | |
| Move / rename detection | ✅ | ⚠️ | ✅ | ✅ | Windows `meta_inode()` returns 0 → moves re-send + delete (receiving a move works) |
| Folder-sync popup (HUD) | ✅ | ✅ | ✅ | N/A | |
| Background sync while closed | ✅ | ✅ | ✅ | ❌ | iOS suspends the app — impossible without a server |

### Locations
| Feature | macOS | Windows | Linux | iOS | Notes |
|---|---|---|---|---|---|
| Host a Location (wizard, rights, cap, marker) | ✅ | ❌ | ✅ | ❌ | Windows: `Root::open` bails "requires Linux or macOS in Phase 1" but the UI is reachable (§3). iOS: by design (`locations::load` returns empty) |
| Mount/drive suggestions in wizard | ✅ | ❌ | ✅ | N/A | `mounts.rs` macOS/Linux only |
| Hosted "Gateway" cards + activity | ✅ | ❌ | ✅ | N/A | |
| Browse / download | ✅ | ✅ | ✅ | ✅ | |
| Upload files / folder | ✅ | ✅ | ✅ | ✅ | iOS also "Upload Photos" |
| New folder / rename / trash | ✅ | ✅ | ✅ | ✅ | |
| Sort (name/size/date) | ✅ | ✅ | ✅ | ❌ | iOS lists by name only |
| Synced-to-a-location folders | ✅ | ✅ | ✅ | ❌ | iOS: undesirable (no background) |

### History & recovery
| Feature | macOS | Windows | Linux | iOS | Notes |
|---|---|---|---|---|---|
| Recents (groups, search) | ✅ | ✅ | ✅ | ✅ | |
| Thumbnails + in-app preview | ❌ | ❌ | ❌ | ✅ | |
| Remove one entry | ❌ | ❌ | ❌ | ✅ | `history::remove_history_entry` is `#[cfg(target_os = "ios")]` |
| Clear all | ✅ | ✅ | ✅ | ✅ | |
| Recoverable files (restore/forget/empty/gauge) | ✅ | ✅ | ✅ | ✅ | |
| Retention settings | ✅ | ✅ | ✅ | ✅ | |

### Settings, diagnostics, updates
| Feature | macOS | Windows | Linux | iOS | Notes |
|---|---|---|---|---|---|
| Theme, sounds, notifications, read receipts, Giphy key | ✅ | ✅ | ✅ | ✅ | |
| Start at login / close-to-tray | ✅ | ✅ | ✅ | N/A | |
| Show folder-sync popup | ✅ | ✅ | ✅ | N/A | |
| Direct-only / wait-for-direct / parallel streams / upload cap / megabits | ✅ | ✅ | ✅ | ✅ | |
| "Prefer Direct Connections" | N/A | N/A | N/A | ⚠️ | iOS-only toggle for `preferDirectP2p`, which the engine never reads (dead) |
| Custom relay | ✅ | ✅ | ✅ | ✅ | desktop has a Restart button; iOS says "close and reopen" |
| Local Network help | ✅ | ⚠️ | ⚠️ | ⚠️ | Win/Linux show a macOS-only "Open Settings" (opens `x-apple.systempreferences:`); iOS has text only and no "relay but LAN peer" nudge |
| Test connection | ✅ | ✅ | ✅ | ✅ | |
| Detailed logging / Export logs / Share diagnostics | ✅ | ✅ | ✅ | ✅ | |
| Diagnostics endpoint + Send test | ✅ | ✅ | ✅ | ❌ | JS bridge handler `diagnosticsTest` exists but Swift never calls it |
| Lab Mode | ✅ | ✅ | ✅ | ❌ | developer-only; fine to omit on iOS |
| Feedback | ✅ | ✅ | ✅ | ✅ | iOS also has a floating-button toggle |
| In-app updates | ✅ | ✅ | ⚠️ | N/A | Linux: designed for AppImage; `.deb` installs likely need a manual reinstall (unverified) |
| Install-location warning | ✅ | N/A | N/A | N/A | |

---

## 2. Ranked parity gaps (worth closing)

Ranked by user impact ÷ effort. Effort: **S** ≈ hours, **M** ≈ 1–2 days, **L** ≈ several days.

| # | Gap | Platforms | Why it matters | Files | Effort |
|---|---|---|---|---|---|
| 1 | **iOS has no Shared Folders screen.** A folder can be joined (Folder Invite sheet, Friends → + → Join Shared Folder) but afterwards there's no way to see its status, find it, pause it, leave it or change its mode. | iOS | Users can join something they then can't manage or leave | new `UI/FoldersView.swift` (list + detail); `nativeBridge.ts` handlers wrapping `listPairs`/`getFolderStatuses`/`setFolderPaused`/`removePair`/`updatePair`/`verifyFolder` + `pairs`/`folderStatuses` snapshots; `Models.swift` `Pair`/`FolderStatus`; entry from Friends or Settings | M (read-only list + pause/leave) → L (full parity incl. roles) |
| 2 | **Local Network banner/button is macOS-only but shows on Windows and Linux.** `lan_path_blocked()` fires there too; the banner says "Enable DropBeam under Local Network" and *Open Settings* opens an `x-apple.systempreferences:` URL that does nothing. | Windows, Linux | Misleading advice exactly when LAN transfers are slow | `src/App.tsx` `LocalNetworkBanner`; `src/views/SettingsView.tsx` (~L416); `src-tauri/src/commands.rs` `open_local_network_settings` (L653) — gate on macOS, give Windows firewall / "Private network" text + `ms-settings:network` link | S |
| 3 | **Windows can open the Locations host wizard but saving always fails** ("Hosting Locations requires Linux or macOS in Phase 1"). | Windows | Dead-end UI | `src/components/LocationSettings.tsx`, `src/views/LocationsView.tsx` (*Share a folder* / *Set up a location* / *Add a location*): hide or explain on Windows. Real hosting = port `locations.rs` `Root` (descriptor-relative I/O) to Windows | S (hide/explain) · L (implement) |
| 4 | **Dead iOS toggle "Prefer Direct Connections"** writes `preferDirectP2p`; nothing in the engine reads `prefer_direct_p2p`. | iOS | A switch that does nothing erodes trust | `UI/SettingsView.swift` L262 (remove), optionally drop the field from `models.rs` | S |
| 5 | **iOS transfer cards lack the path badge, the wait-for-direct detail and "Send over relay anyway".** | iOS | With *Wait for a Direct Link* on, a parked send has no explanation or escape | `UI/SendView.swift` `TransferCard`; `Models.swift` `Transfer` (decode `detail`, `connDetail`); add `forceRelay` bridge handler → `api.forceRelay` | S |
| 6 | **iOS can't dismiss finished/failed transfers.** | iOS | Send tab fills with stale cards until relaunch | `UI/SendView.swift` (swipe/✕ on non-active cards); `nativeBridge.ts` `removeTransfer` handler → `st().removeTransfer` | S |
| 7 | **iOS "Have a code?" accepts only Quick Send codes;** a friend/folder/device code gets an error whose wording names desktop places ("Shared Folders → Accept invite", "Settings → Devices"). | iOS | Scan-anything convenience desktop already has | `nativeBridge.ts` `receiveWithCode` → use `st().openCode` and handle folder/device kinds natively; `src/lib/codes.ts` `CODE_HOME` needs iOS wording (`Friends → + → Join Shared Folder`) | S |
| 8 | **iOS can't send a folder** to a friend or by Quick Send. | iOS | Common for photo albums / project folders | `UI/SendView.swift` + `FriendsView.swift` (add "Folder" source); `Bridge.pickFiles` → `NativeFolderPicker.pick()` (already used by `browserUpload`) | S–M |
| 9 | **iOS: no Pause, no Verify copy, no integrity details** (Send tab and History). | iOS | Parity for big transfers and trust signals | `UI/SendView.swift`, `UI/HistoryView.swift`; bridge handlers for `api.pauseTransfer`, `api.verifyTransfer`, `api.cancelVerify`; decode `integrity`/`verify` in `Models.swift` | M |
| 10 | **Linux tray can't open the quick menu** (Tauri doesn't deliver tray click events on Linux). | Linux | The "always in the tray" workflow is Open/Quit only | `src-tauri/src/lib.rs` `build_tray` — add menu items (*Send a file…*, *Receive with a code…*, recent friends) under `#[cfg(target_os = "linux")]` | S |
| 11 | **Desktop can't remove a single History entry** (iOS can). | macOS, Win, Linux | Parity; privacy cleanup | `src-tauri/src/history.rs` (drop the `cfg(ios)` on `remove_history_entry`), `lib.rs` handler cfg, `src/views/HistoryView.tsx` row menu | S |
| 12 | **"Add person" on an existing folder only makes a code;** inviting a friend directly exists only at creation. | macOS, Win, Linux | Obvious next step is missing | `src/views/FoldersView.tsx` `addPerson` → friend picker using existing `api.inviteFriendToFolder` (as `PairingModal.tsx` L61) | S |
| 13 | **iOS: can't paste an image into chat.** | iOS | Screenshots are the #1 thing people paste | `UI/Chat/ChatComposer.swift` (paste handling / `PasteButton`), save via existing `save_pasted_image` command through a bridge handler, then `stageChatFiles` | M |
| 14 | **No "send with DropBeam" from other apps on iOS and Linux** (macOS Services, Windows right-click exist). | iOS, Linux | Sending starts where the file is | iOS: new Share Extension target in `gen/apple/project.yml` + app-group handoff into `pendingSend` (L). Linux: `.desktop` `MimeType`/Nautilus script passing a path (single-instance forwarding already exists in `lib.rs`) (M) | L / M |
| 15 | **Unread badge on the app icon** only on macOS. | iOS, Windows | Missed messages | iOS: `UNUserNotificationCenter.setBadgeCount` from `Bridge.unread` (S). Windows: taskbar overlay icon in `commands.rs` `set_unread_badge` (M) | S / M |
| 16 | Desktop chat: no pins, no chat-list search, no mark-read from list (iOS has all three). | macOS, Win, Linux | Nice-to-have parity | `src/views/ChatView.tsx` list pane | S–M |
| 17 | Windows move detection re-sends moved files. | Windows | Slow reorganizations of big folders | `src-tauri/src/sync.rs` `meta_inode` — `GetFileInformationByHandle` file index | M |
| 18 | iOS Locations: no sort; desktop Friends: no search. | iOS / desktop | Minor | `UI/LocationsView.swift` `BrowserView` (pass `sort`), `src/views/FriendsView.tsx` | S |
| 19 | iOS Diagnostics: no *Send test* / endpoint. | iOS | Support tooling | `UI/SettingsView.swift` `DiagnosticsView` → existing `diagnosticsTest` handler | S |
| 20 | Linux `.deb` in-app updates. | Linux | deb users silently stay behind | `release.yml` / updater config — confirm plugin-updater deb support or show "download the new .deb" | M |

### Gaps that are impossible or undesirable (don't chase)
| Gap | Platform | Reason |
|---|---|---|
| Background receiving, background folder sync, notifications while closed | iOS | iOS suspends the app; there's no server to wake it (no push). DropBeam is serverless by design. |
| Hosting a Location | iOS | Phones are clients only (`locations::load` short-circuits on iOS); the host must stay awake and reachable. |
| Synced-to-a-location folders | iOS | One-way continuous sync needs a background watcher. |
| Creating / owning a shared folder with full mirror semantics | iOS | Possible only in foreground; a phone as the source of truth for a mirror is fragile. Joining + a status/leave screen (gap #1) is the right scope. |
| "Save to…" per delivery, custom download folder | iOS | Sandbox: files live in the app's Documents (visible in Files). |
| Start at login / close-to-tray / tray menu / HUD / floating card / taskbar progress | iOS | Desktop-shell concepts. |
| Drag-onto-tray-icon send | Windows, Linux | Relies on macOS NSStatusItem drag; Windows has right-click instead. |
| In-app updater | iOS | App Store / TestFlight own updates. |
| Lab Mode | iOS | Developer test surface; the operator drives desktop devices. |

---

## 3. UI that exists but is broken, unreachable or dead

| Where | What | Evidence |
|---|---|---|
| Desktop (Win/Linux) | Local Network banner + Settings *Open Settings* button target macOS only | `App.tsx` `LocalNetworkBanner`; `SettingsView.tsx` ~L416; `commands.rs` L653 opens `x-apple.systempreferences:`; `iroh_net.rs` L1379 `lan_path_blocked()` has no OS gate |
| Desktop (Windows) | Locations host wizard reachable, always fails on save | `locations.rs` L314 `bail!("Hosting Locations requires Linux or macOS in Phase 1")`; `LocationSettings.tsx` rendered for every desktop (`SettingsView.tsx` L257 `!MOBILE_UI`) |
| Desktop (Windows) | Wizard's drive/NAS suggestions always empty | `mounts.rs` L162 returns `vec![]` off macOS/Linux |
| iOS | Joined shared folders are invisible afterwards | `NativeSheets.swift` `FolderInviteSheet`, `FriendsView.swift` L38 "Join Shared Folder"; no Swift view lists pairs; snapshots in `nativeBridge.ts` don't include `pairs`/`folderStatuses` |
| iOS | "Prefer Direct Connections" toggle is inert | `SettingsView.swift` L262 → `preferDirectP2p`; `prefer_direct_p2p` is read nowhere outside `models.rs` |
| iOS | Wait-for-direct has no escape | setting at `SettingsView.swift` L266; no `forceRelay` bridge handler |
| iOS | Code-error messages point at desktop screens | `src/lib/codes.ts` `CODE_HOME` ("Shared Folders → Accept invite", "Settings → Devices") shown via `store.receiveCode` |
| Bridge | Handlers nothing calls from Swift: `diagnosticsTest`, `locationsList` (Swift uses `locationsRefresh`), `needsName` (Swift uses the `needsName` snapshot) | `src/lib/nativeBridge.ts` |
| Desktop commands | Registered but never invoked by any UI: `iroh_node_id`, `verify_folders` (`api.verifyFolders`), `create_friend` (only via the unused `store.createFriend`) | `lib.rs` handler list; `src/lib/api.ts` L479/L528; `store.ts` L1554 |
| Desktop api | `api.sendFiles` / `api.receiveFiles` invoke `send_files` / `receive_files`, which aren't registered commands (dead, would fail if called) | `src/lib/api.ts` L412–413 |
| Settings (all) | `direct_mode` (only echoed into the diagnostics export), `prefer_direct_p2p` and `custom_relay_pass` persist but no engine code acts on them (iroh is the only transport; the relay has no password) | `models.rs` `Settings`; `commands.rs` L516; grep of `src-tauri/src` |
| Web phone UI | `src/mobile/**` and every `MOBILE_UI` branch in desktop views (e.g. SettingsView's mobile Lab Mode / diagnostics endpoint) never render on iOS because the native shell is active | `src/App.tsx` (`MobileApp bridgeOnly`), `nativeShell.ts` — abandoned, safe to delete later |
| Linux | Tray quick menu unreachable (click events unsupported) | `lib.rs` `build_tray` `on_tray_icon_event` |
