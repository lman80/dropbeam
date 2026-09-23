# DropBeam — UI inventory

Every screen, sheet, modal, popover and banner, per platform, with how you get there. It's a checklist for UI sweeps. File paths are relative to the repo root. Feature behaviour is described in [FEATURES.md](FEATURES.md); platform gaps are in [PARITY.md](PARITY.md).

- **Desktop** = macOS, Windows, Linux. It's one React bundle (`src/`), rendered by four Tauri windows (`main`, `popover`, `hud`, `receive`; see `src-tauri/tauri.conf.json`, routed in `src/main.tsx` by window label). Platform differences inside it are noted inline.
- **iOS** = native SwiftUI in `src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/UI/**`. The React app runs hidden as a data bridge only. `src/mobile/**` and the `MOBILE_UI` branches in `src/views/*` are the **abandoned web phone UI** — never shown, so don't sweep them.

---

## Desktop — main window (`src/App.tsx`)

### Chrome and global overlays
| # | Surface | Component | Entry point / trigger |
|---|---|---|---|
| D1 | Title bar (traffic lights / drag region) | `components/TitleBar.tsx` | always |
| D2 | Sidebar nav: Send & Receive · Friends · Chat · Locations · Shared Folders · History · Settings · **Feedback** + "This device" footer | `components/Sidebar.tsx` | always; badges: active transfers (Send), unread (Chat) |
| D3 | Loading splash (beam logo) | `App.tsx` | until the store is `ready` |
| D4 | **Install-location banner** (macOS) | `App.tsx` `InstallBanner` | app running translocated / from Downloads (`macos_install_hint`) |
| D5 | **Local Network banner** + *Open Settings* | `App.tsx` `LocalNetworkBanner` | `lan_network_blocked()` polled every 15 s (shows on every desktop OS — see PARITY §3) |
| D6 | **First-run name dialog** "What should people call you?" | `App.tsx` `NameSetupModal` | first launch (`localStorage dropbeam.namedSelf` unset) |
| D7 | ↳ Join account dialog | `components/DevicesPanel.tsx` `JoinAccountModal` | D6 → "Already use DropBeam on another device? Link it" |
| D8 | **Send-to chooser** (My Devices / Friends / "Share with a code or QR") | `components/SendToChooser.tsx` | any pending send: drop on window, Photos/Files/Choose a folder, Windows right-click, macOS Services, second-instance launch (`open-file-send`) |
| D9 | **Incoming shared-folder invite** prompt | `components/FolderInviteModal.tsx` | `folder-invite://incoming` from a friend |
| D10 | **QR scanner** (camera, drop screenshot, *Scan from image…*, *Paste the code*) | `components/QrScanner.tsx` | every `ScanCodeButton` (`components/CodeQr.tsx`); also `PopoverCodeHandoff` in `App.tsx` when the tray menu asks to scan |
| D11 | Enlarged QR overlay | `components/CodeQr.tsx` (`big` state) | click any QR tile |
| D12 | Toasts | `components/Toasts.tsx` | any `store.toast` |
| D13 | Error-boundary fallback per region | `components/ErrorBoundary.tsx` | a render crash in that region |

### Send & Receive (`src/views/SendView.tsx`)
| # | Surface | Component | Entry point |
|---|---|---|---|
| S1 | Drop zone (*Drag files here to send* / *Drop to send*) + Photos / Files | `components/DropZone.tsx` | Sidebar → Send & Receive |
| S2 | *Choose a folder* button (Windows/Linux only) | `SendView.tsx` | S1 area |
| S3 | *Have a code? Receive files* inline form + *Scan a QR code* | `SendView.tsx` | button under the drop zone |
| S4 | Transfer cards: offer (Accept/Decline), Quick Send waiting (QR + code + copy), progress (speed/ETA toggles), park line + *Send over relay anyway*, completed (Show in folder / Open folder, summary), failed/paused (Retry/Resume), **Verify copy** panel, integrity `<details>`, ConnInspector pill | `components/TransferCard.tsx`, `IntegrityDetails.tsx`, `ConnInspector.tsx`, `bits.tsx` | list below S1–S3 |
| S5 | Empty state | `SendView.tsx` / `bits.tsx EmptyState` | no transfers |

### Friends (`src/views/FriendsView.tsx`)
| # | Surface | Component | Entry point |
|---|---|---|---|
| F1 | My devices section + *Add a device* | `FriendsView.tsx` | Sidebar → Friends |
| F2 | ↳ Add a device dialog (QR, waiting, *Scan the other device instead*, copy) | `DevicesPanel.tsx` `AddDeviceModal` | F1 → Add a device |
| F3 | **You** card: avatar (change/remove), name edit, your code QR + copy | `FriendsView.tsx` `YouCard` | Friends |
| F4 | Friend card: presence dot, rename inline, status + ConnInspector + re-probe, **Check**, Message, **Send**, auto-accept toggle, **Invite** (inline `InvitePanel` QR), remove (confirm row) | `FriendsView.tsx` `FriendCard`, `InvitePanel` | Friends list |
| F5 | **Add friend** dialog (paste code, scan QR) | `FriendsView.tsx` `AddFriendModal` | *Add friend* (header) / empty state *Add a friend* |
| F6 | Empty state "No friends yet" | `FriendsView.tsx` | no friends |

### Chat (`src/views/ChatView.tsx`)
| # | Surface | Component | Entry point |
|---|---|---|---|
| C1 | Conversation list pane (avatar, online dot, last message, unread) | `ChatView.tsx` | Sidebar → Chat |
| C2 | "Select a conversation" placeholder / "No one to chat with yet" empty state | `ChatView.tsx` | no thread open / no friends |
| C3 | Conversation header: name, presence/typing, 🔍 search toggle, *Shared folder* button | `ChatView.tsx` `Conversation` | C1 row, friend card Message, notification |
| C4 | Search bar (query, n of m, ↑/↓, Esc) | `ChatView.tsx` | C3 🔍 |
| C5 | Message bubbles: text/links, GIF, file cards (progress, retry, open), reactions chips, quote, edited, Delivered/Read | `ChatView.tsx`, `ChatTransferProgress.tsx`, `TransferCard.tsx` | thread |
| C6 | Hover actions: react tray, reply, ⋯ menu (Copy / Edit / Unsend) | `ChatView.tsx` | hover a bubble |
| C7 | Reply bar / Editing bar | `ChatView.tsx` | C6 Reply / Edit |
| C8 | Staged attachment chips | `ChatView.tsx` | 📎, drop onto chat, paste image |
| C9 | GIF picker | `components/GifPicker.tsx` | ✨ (only with a Giphy key) |
| C10 | Emoji picker | `ChatView.tsx` | ☺ in composer |
| C11 | Image lightbox | `ChatView.tsx` | click an image/GIF |
| C12 | Shared-folder activity rows | `ChatView.tsx` (+ `store.ts` folder-synced logging) | automatic in a friend's thread |

### Locations (`src/views/LocationsView.tsx`)
| # | Surface | Component | Entry point |
|---|---|---|---|
| L1 | Header: *Refresh*, *Share a folder* (→ Settings) | `LocationsView.tsx` | Sidebar → Locations |
| L2 | "Synced to a location" section: synced-folder tiles (Sync now / Pause / Open folder / Remove + confirm) | `components/SyncedFolders.tsx` | when any synced folder exists |
| L3 | *Sync a folder here* call-to-action | `SyncedFolders.tsx` `SyncFolderToolbar` | when friends share ≥1 location |
| L4 | **Sync a folder** 3-step sheet | `SyncedFolders.tsx` `SyncSheet` | L2 *Sync a folder* / L3 |
| L5 | Friend location tiles (presence, free space, rights) + unavailable-devices `<details>` | `LocationsView.tsx` | page body |
| L6 | Empty state "A place for everything" → *Set up a location* | `LocationsView.tsx` | no shared locations |
| L7 | **File browser**: breadcrumbs, Download / Upload files / Upload folder / New folder / Rename / Trash / Refresh, filter, sort, table, paging, drag-to-upload highlight | `components/FileBrowser.tsx` | click an L5 tile |
| L8 | Browser action dialog (New folder / Rename / Move to trash + per-item results) | `FileBrowser.tsx` `actionSheet` | L7 buttons |
| L9 | "Shared from this device" gateway cards (reachability, free space, members, live receiving line, last activity) + empty state | `LocationsView.tsx` `SharedFromThisDevice` | bottom of page (hosting: macOS/Linux) |

### Shared Folders (`src/views/FoldersView.tsx`)
| # | Surface | Component | Entry point |
|---|---|---|---|
| SF1 | Header: *Accept invite*, *New folder*; empty state | `FoldersView.tsx` | Sidebar → Shared Folders |
| SF2 | **Pairing modal — create** (folder, their name, mode, invite friends) → invite QR result | `components/PairingModal.tsx` (`mode="create"`) | SF1 *New folder* / *Create one* |
| SF3 | **Pairing modal — accept** (folder, invite code + scan) | `PairingModal.tsx` (`mode="accept"`) | SF1 *Accept invite*; `openCode` with a `dropbeam1:` code |
| SF4 | Folder card: members (role seg Editor/Viewer, remove), *Add person*, mode chips, status line, stop-transfer, queued list, completion summary, "no longer shares" / viewer banners, Open folder, Pause/Resume, ⚙ | `FoldersView.tsx` `FolderCard`, `Member`, `RoleSeg`, `Banner` | list |
| SF5 | Remove-member / cancel-invite confirm row | `FoldersView.tsx` | SF4 member ✕ |
| SF6 | Folder settings panel (sound, total sync, two-way, delete after delivery + Trash/Permanent, History, Verify + `VerifyBanner`, Show invite, Unpair/Leave confirm) | `FoldersView.tsx` | SF4 ⚙ |
| SF7 | Folder **Invite modal** (QR + copy) | `FoldersView.tsx` `InviteModal` | SF4 *Add person*, SF6 *Show invite* |

### History (`src/views/HistoryView.tsx`)
| # | Surface | Component | Entry point |
|---|---|---|---|
| H1 | Tabs Recents / Recoverable files; *Clear list* | `HistoryView.tsx` | Sidebar → History |
| H2 | Recents: search, date groups, rows (icon, direction chip, locality, integrity `<details>`, Show in folder) + empty/no-match states | `HistoryView.tsx` `Recents`, `RecentRow` | H1 Recents |
| H3 | Recoverable: storage gauge, *Free up space* confirm, per-folder sections (expand, Empty confirm), items (Restore / Forget) | `views/RecoverableFilesView.tsx` | H1 Recoverable; SF6 *History* (deep-link via `focusFolderHistory`) |

### Settings (`src/views/SettingsView.tsx`)
| # | Surface | Component | Entry point |
|---|---|---|---|
| ST1 | **Devices** panel (list, ⋯ Remove from account, Sync now, Add a device, Join…, Remove this device) | `components/DevicesPanel.tsx` | Sidebar → Settings (top) |
| ST2 | ↳ Add a device / Join your account / Link this device (show code) dialogs | `DevicesPanel.tsx` `AddDeviceModal`, `JoinAccountModal`; `components/LinkDeviceModal.tsx` `LinkDeviceModal` | ST1 buttons / *Show a code instead* |
| ST3 | **Locations** settings: hosted rows (Edit / Stop sharing / Advanced), *Add a location*, Recent activity | `components/LocationSettings.tsx` | Settings; L1 *Share a folder*; L6; L9 *Manage* |
| ST4 | ↳ **Add-a-location wizard** (Where → Name → Who, Advanced cap) | `LocationSettings.tsx` `AddLocationWizard` | ST3 *Add a location* |
| ST5 | ↳ Edit location form | `LocationSettings.tsx` | ST3 *Edit* |
| ST6 | Profile, Downloads (Save to, Clear transfer cache), Appearance, Behavior (background, close-to-tray, notifications, read receipts, Giphy key, sounds, sync popup) | `SettingsView.tsx` | Settings |
| ST7 | Connection (Test direct connection, Local network access, direct-only, wait-for-direct, parallel streams, upload limit presets, megabits) + "How transfers connect" | `SettingsView.tsx` | Settings |
| ST8 | Recoverable files (keep for, storage per folder, Free up space) | `SettingsView.tsx` | Settings |
| ST9 | Custom relay (URL + Restart) | `SettingsView.tsx` | Settings |
| ST10 | Updates (version, Check, Install & restart progress, *Get the latest from GitHub*) | `SettingsView.tsx`, `lib/updater.ts` | Settings |
| ST11 | Diagnostics (Detailed logging + Restart, Export logs, Share background diagnostics, endpoint + Send test) | `SettingsView.tsx` | Settings |
| ST12 | Lab Mode (enable, operator ID + scan QR, this device's ID + copy + QR) | `SettingsView.tsx` | Settings |
| ST13 | Feedback panel | `src/vendor/superfeedback.js` | Sidebar → Feedback |

## Desktop — other windows

| # | Window | Component | Entry point |
|---|---|---|---|
| W1 | **Quick menu / popover**: header (Open DropBeam, Quit, Close), friend search, friend rows (click → pick files → send; macOS drop target), recent transfers (≤4), *Send a file*, ↓ receive-with-code form + scan (hands off to main) | `src/windows/Popover.tsx` | macOS menu-bar click or drag-hover; Windows tray left-click; **not reachable on Linux** |
| W2 | Tray context menu (Open DropBeam / Quit DropBeam) | `src-tauri/src/lib.rs` `build_tray` | right-click the tray icon (on Linux this menu is the only tray interaction) |
| W3 | **HUD** — folder-sync popup (folder, direction, progress, Dismiss) | `src/windows/Hud.tsx` | a shared folder moving bytes (Settings → *Show the folder-sync popup*) |
| W4 | **Floating transfer card** — incoming offer (Accept / Save to… menu / Decline), progress ring, *Sent ✓* / Done | `src/windows/ReceiveCard.tsx` | one-off sends/receives and manual-accept offers |
| W5 | OS surfaces: notifications, Dock badge (macOS), Dock/taskbar progress, Finder Services item (macOS), Explorer "Send with DropBeam" (Windows) | `lib.rs`, `commands.rs`, `lib/taskbar.ts`, `mac_service.rs`, `windows/hooks.nsh` | system |

---

## iOS — native SwiftUI (`…/NativeUIPlugin/UI`)

### Root and global (`RootView.swift`)
| # | Surface | View | Entry point |
|---|---|---|---|
| I1 | Tab bar: Send · Friends · Chat (unread badge) · History · Settings | `RootView` | launch |
| I2 | No-network banner | `RootView` `safeAreaInset` | `NWPathMonitor` unsatisfied |
| I3 | Toast capsule | `RootView` overlay | `Bridge.showToast` |
| I4 | Error alert "Couldn't complete that" | `RootView` `.alert` | any bridge error / `error` event |
| I5 | Media preparation overlay (*Preparing photo…* + Cancel) | `RootView.swift` `MediaPreparationOverlay` | any Photos/Files pick |
| I6 | **Onboarding** sheet (name, *Scan Code*) | `NativeSheets.swift` `OnboardingSheet` | `needsName` snapshot true |
| I7 | ↳ Join account scanner | `DevicesView.swift` `JoinAccountSheet` → `QRScannerSheet` | I6 *Scan Code* |
| I8 | **Send to** sheet (My Devices / Friends / Quick Send) | `NativeSheets.swift` `SendToSheet` | `pendingSend` non-empty (after Send-tab pick) |
| I9 | **Folder Invite** sheet (Accept & Import / Decline) | `NativeSheets.swift` `FolderInviteSheet` | `folder-invite://incoming` event |
| I10 | **QR scanner sheet** (VisionKit camera, *Paste code instead*) | `NativeSheets.swift` `QRScannerSheet` | reused by I7, S-I2, F-I2, F-I3, DV-I2/I3 |
| I11 | Floating feedback button + feedback panel + screenshot markup | `SuperFeedback.swift` `SFOverlayView`, `SFPanel`, `SFMarkupEditor` | Settings → Feedback (toggle / *Send Feedback*) |

### Send tab (`SendView.swift`)
| # | Surface | View | Entry point |
|---|---|---|---|
| S-I1 | Hero card with **Photos** / **Files** | `SendView` | Send tab |
| S-I2 | *Have a code?* field + QR icon (→ I10 "Receive Files") + Receive | `SendView` | Send tab |
| S-I3 | Transfer cards: cancel, progress/ETA, Quick Send QR + code + Copy, Accept Files / Decline, Share, error + Retry/Resume | `SendView.swift` `TransferCard`, `QRCodeView` | list |
| S-I4 | Empty state "Nothing in flight" | `SendView` | no transfers |

### Friends tab (`FriendsView.swift`, `LocationsView.swift`, `DevicesView.swift`)
| # | Surface | View | Entry point |
|---|---|---|---|
| F-I1 | List: *Locations* link, My Devices, Friends, search | `FriendsView` | Friends tab |
| F-I2 | **+** menu → *Add Friend* scanner | `FriendsView.swift` `AddFriendSheet` → I10 | F-I1 + |
| F-I3 | **+** menu → *Join Shared Folder* scanner | `FriendsView` → I10 | F-I1 + |
| F-I4 | "Link Your Other Devices" link (empty My Devices) → DevicesView | `FriendsView` | F-I1 |
| F-I5 | **Friend detail**: Send Files (Photos/Files dialog), Message, Check (result line), Accept automatically, Rename alert, Browse Locations, Remove confirm | `FriendsView.swift` `FriendDetailView` | tap a friend; also Chat header (sheet) |
| F-I6 | **Locations** list: per-friend sections, offline/error lines + Retry, "Other friends" reasons card, empty states, refresh | `LocationsView.swift` `LocationsView` | F-I1 *Locations*; F-I5 *Browse Locations* (filtered) |
| F-I7 | **Browser**: search, Select, ••• (New Folder / Upload Photos / Files / Folder / Refresh), row dialog (Download / Rename / Move to Trash), selection bar, name alert, trash confirm, banner, *Show More* | `LocationsView.swift` `BrowserView` | F-I6 location card; folders push nested BrowserView |

### Chat tab (`UI/Chat/*`)
| # | Surface | View | Entry point |
|---|---|---|---|
| C-I1 | Chats list: pinned grid, rows, swipe Pin/Read, context menu, search, empty state | `ChatsView.swift` `ChatsView`, `ChatRow`, `PinnedGrid` | Chat tab |
| C-I2 | **New Message** friend picker | `ChatsView.swift` `ChatFriendPicker` | C-I1 ✎ |
| C-I3 | **Conversation**: header (avatar → friend sheet), search toggle + footer (n of m), bubbles, typing bubble, *New Messages* pill, Edited/Delivered/Read | `ConversationView.swift`, `ChatBubble.swift` | C-I1 row, C-I2, F-I5 *Message*, notification tap |
| C-I4 | **Tapback overlay** (reactions + more emoji; Reply / Copy / Edit / Save or Share / Undo Send) | `ChatTapback.swift` `TapbackOverlay` | long-press a bubble |
| C-I5 | Composer: reply/edit bar, **+** menu (Photos / Files / GIFs), staged thumbnails, send | `ChatComposer.swift` | C-I3 |
| C-I6 | GIF picker | `ChatComposer.swift` `ChatGifPicker` | C-I5 + → GIFs (only with a Giphy key) |
| C-I7 | Attachments (media grid, documents, *Not Delivered · Retry*) + **paged media viewer** | `ChatAttachment.swift` `ChatAttachment`, `PagedMediaViewer`, `MediaViewer` | file messages; tap media |
| C-I8 | Friend detail sheet | `FriendDetailView` in a sheet | C-I3 header |

### History tab (`HistoryView.swift`)
| # | Surface | View | Entry point |
|---|---|---|---|
| H-I1 | Segmented Recents / Recoverable, search, ••• *Clear History* confirm | `HistoryView` | History tab |
| H-I2 | Recents: day groups, thumbnail rows, context menu (Share / Copy Name / Remove), empty state | `HistoryView` | H-I1 Recents |
| H-I3 | Full-screen media viewer | `ChatAttachment.swift` `MediaViewer` via `fullScreenCover` | tap a photo/video row |
| H-I4 | Recoverable: gauge + *Empty All*, per-folder cards (*Empty*), items ••• (Restore / Delete Forever), confirms | `HistoryView.swift` `RecoverableView` | H-I1 Recoverable |

### Settings tab (`SettingsView.swift`, `DevicesView.swift`)
| # | Surface | View | Entry point |
|---|---|---|---|
| ST-I1 | Profile card (tap avatar → photo picker) | `SettingsView` | Settings tab |
| ST-I2 | **Profile**: avatar (context *Remove Picture*), name alert, your code QR, Copy / Share | `SettingsView.swift` `ProfileView`, `InviteQRCode` | ST-I1 name row |
| ST-I3 | **Devices**: hero, list (••• / context Remove), Add a Device, Sync Now, Remove This iPhone; or *Already use DropBeam?* (Scan Code / Show a Code Instead) + *New device?* | `DevicesView.swift` `DevicesView` | Settings → My Devices; F-I4 |
| ST-I4 | ↳ **Add a Device** sheet (QR, waiting, linked ✓, *Scan the Other Device Instead*, Copy) | `DevicesView.swift` `AddDeviceSheet` | ST-I3 |
| ST-I5 | ↳ Join account scanner | `JoinAccountSheet` | ST-I3 *Scan Code* |
| ST-I6 | ↳ **Link This Device** (show code) | `SettingsView.swift` `LinkThisDeviceSheet` | ST-I3 *Show a Code Instead* |
| ST-I7 | General group: Appearance menu, Sounds, File/Chat Notifications, Read Receipts, background note, **Giphy Key** (→ text editor) | `SettingsView`, `TextSettingView` | Settings |
| ST-I8 | Transfers group: *Prefer Direct Connections* (dead — PARITY §3), Direct Only, Wait for a Direct Link, Parallel Streams, **Upload Limit** (→ editor), Megabits, **How Transfers Connect** (→ Test Connection + explainer), Clear Transfer Cache confirm | `SettingsView`, `UploadLimitView`, `ConnectionInfoView` | Settings |
| ST-I9 | Advanced: **Custom Relay** editor, **Diagnostics** (Detailed Logging, Share Background Diagnostics, Export Logs → share sheet), **Recoverable Files** (keep-for / storage pickers, Free Up Space confirm) | `TextSettingView`, `DiagnosticsView`, `RecoverySettingsView` | Settings |
| ST-I10 | Feedback group (button toggle, *Send feedback…*, *Send Feedback*) + About (version) | `SettingsView`, `SuperFeedback.swift` | Settings |

### iOS system surfaces
Photos picker (`ios_media.rs` `pick_photos`), document picker for files and folders (`NativeFolderPicker.swift`), share sheet (`ios_media.rs` `share_files`), camera and Local Network permission prompts, and notification banners (a tap opens the chat through `lib/chatNotifications.ts`).

---

## Not present on a platform (so there's nothing to sweep there)
- **iOS:** no Shared Folders screen, no Locations hosting or Synced-folder screens, no Lab Mode, no updater, no tray/HUD/floating card.
- **Linux:** the popover (W1) can't be opened (tray click events aren't delivered).
- **Windows:** L9 and ST3–ST5 render, but hosting can't succeed there.
