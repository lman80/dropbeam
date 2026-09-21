# Mobile slice 1 implementation report

Implemented on branch `ios`, offline, using installed dependencies. No commits,
pushes, version changes, or edits to other workers’ files. The existing Xcode
scheme modification was present before this work and was left alone.

## Files created

| File / entry point | Purpose |
| --- | --- |
| [MobileApp.tsx:13](/Users/ashtonmiller/DropBeam-ios/src/mobile/MobileApp.tsx:13) | Top-level mobile routing and local Friends/Settings push stack |
| [SendScreen.tsx:29](/Users/ashtonmiller/DropBeam-ios/src/mobile/SendScreen.tsx:29) | Send list, receive sheet, transfer rows and details |
| [useSend.ts:8](/Users/ashtonmiller/DropBeam-ios/src/mobile/useSend.ts:8) | Picker/list/receive presentation adapter using existing store actions |
| [FriendsScreen.tsx:13](/Users/ashtonmiller/DropBeam-ios/src/mobile/FriendsScreen.tsx:13) | Friends list, friend detail, add-friend sheet |
| [SettingsScreen.tsx:15](/Users/ashtonmiller/DropBeam-ios/src/mobile/SettingsScreen.tsx:15) | Settings, Profile, Appearance, Messages, Connection, Relay, Diagnostics, Lab Mode |
| [useSettings.ts:8](/Users/ashtonmiller/DropBeam-ios/src/mobile/useSettings.ts:8) | Existing settings persistence, cache cleanup, diagnostics/export actions |
| [Onboarding.tsx:5](/Users/ashtonmiller/DropBeam-ios/src/mobile/Onboarding.tsx:5) | Non-dismissible first-run sheet |
| [shared.tsx:9](/Users/ashtonmiller/DropBeam-ios/src/mobile/shared.tsx:9) | Presence refresh, friend avatar, clipboard/error adapters, editable-name alert |
| [helpers.ts:2](/Users/ashtonmiller/DropBeam-ios/src/mobile/helpers.ts:2) | Pure presence labels and friend-code classification |
| [kit/Screen.tsx:7](/Users/ashtonmiller/DropBeam-ios/src/mobile/kit/Screen.tsx:7) | NavBar and scroll-owning Screen |
| [kit/controls.tsx:4](/Users/ashtonmiller/DropBeam-ios/src/mobile/kit/controls.tsx:4) | Grouped list, rows, controls, avatar, progress and empty state |
| [kit/overlays.tsx:8](/Users/ashtonmiller/DropBeam-ios/src/mobile/kit/overlays.tsx:8) | Focus-trapped Sheet, ActionSheet, Alert, ContextMenu |
| [kit/kit.css:1](/Users/ashtonmiller/DropBeam-ios/src/mobile/kit/kit.css:1) | Scoped system tokens, typography, layouts, safe areas, accessibility preferences |
| [kit/index.ts:1](/Users/ashtonmiller/DropBeam-ios/src/mobile/kit/index.ts:1) | Public exports |
| [kit/README.md:1](/Users/ashtonmiller/DropBeam-ios/src/mobile/kit/README.md:1) | Kit integration/API guide for following slices |
| [mobile-helpers.test.ts:5](/Users/ashtonmiller/DropBeam-ios/tests/mobile-helpers.test.ts:5) | Two pure-helper tests, with no Tauri imports |
| [IMPLEMENTATION.md:1](/Users/ashtonmiller/DropBeam-ios/src/mobile/IMPLEMENTATION.md:1) | This report |

## Existing files changed

| File / entry point | Change |
| --- | --- |
| [App.tsx:179](/Users/ashtonmiller/DropBeam-ios/src/App.tsx:179) | MOBILE_UI renders MobileApp; phone onboarding at line 203; TitleBar hidden on mobile |
| [mobile.css:1](/Users/ashtonmiller/DropBeam-ios/src/mobile.css:1) | Imports kit CSS, removes redundant font-scale reset; shell/inset rules start at line 361 |
| [SendToChooser.tsx:60](/Users/ashtonmiller/DropBeam-ios/src/components/SendToChooser.tsx:60) | Mobile-only grouped recipient sheet |
| [MobileFileSheet.tsx:9](/Users/ashtonmiller/DropBeam-ios/src/components/MobileFileSheet.tsx:9) | Explicit source adapter; ActionSheet at line 45 |
| [LinkDeviceModal.tsx:28](/Users/ashtonmiller/DropBeam-ios/src/components/LinkDeviceModal.tsx:28) | Mobile Link New Device progress/error sheet; account-link QR sheet at line 68 |
| [QrScanner.tsx:63](/Users/ashtonmiller/DropBeam-ios/src/components/QrScanner.tsx:63) | Mobile camera/paste sheet, preserving decoder and media cleanup |

MobileHeader, desktop view files, ChatView, HistoryView, RecoverableFilesView,
LocationsView, FileBrowser, TransferCard, MobileTabBar, src-tauri, api.ts and
store.ts were not edited. Existing desktop JSX branches remain unchanged.
The imported kit has no module-level DOM effects and its CSS requires html.mobile.

## Kit API

`Screen(title, leading, trailing)` owns scrolling and measures when the large title
has passed the sticky 44pt NavBar. Leading accepts a back title/callback or a node.
`Section(title, footer)` wraps grouped `List` cells. `Row` supports an icon square
or avatar, title/subtitle/value, chevron/toggle/checkmark/none, destructive/tint,
disabled/pressed states, and a separate trailing control. `ProgressRow` adds a 2pt
track. `Button` supports plain text and filled sheet CTA; Switch uses a green 51×31
visual with a 44pt target. SegmentedControl, SearchField, TextField, Avatar,
Badge and a button-free EmptyState are public exports.

Sheet has medium/large sizes, grabber, Cancel/title/primary header, body scroll,
safe-area padding and optional non-dismissibility. ActionSheet has grouped actions
and a separate Cancel group. Alert supports a text field and explicit save/cancel.
ContextMenu supports a 400ms touch hold, mouse click and keyboard activation.
Native HTML dialogs provide focus trapping and background inertness; a shared
counter handles nested scroll locks. Reduced motion/transparency and font scaling
are supported. All targets are at least 44pt, except the explicitly requested
36pt search field. The README documents all props and composition conventions.

## Screen and action mapping

| Previous element / action | New destination / behavior |
| --- | --- |
| Send hero/drop zone and photo/file buttons | Send → two grouped Photos and Videos / Files rows; explicit native source selection |
| Inline “Have a code?” form | Receive group → Receive files sheet, autofocus Code field and Done; failed receive keeps the code and shows inline feedback |
| Standalone transfer cards | Transfers group → compact ProgressRows, first-file glyph, status/counters/speed, cancel/retry/resume actions |
| Waiting transfer QR/code block | Transfer row shows code; tap opens QR + Copy Code sheet |
| Completed transfer “Show” | Native api.shareFiles; received outDir paths or original sender paths from the existing retry cache |
| Incoming offer acceptance | Transfer row → Accept Files / Decline grouped rows |
| Parked direct-link escape | Transfer sheet → Send over relay anyway |
| Transfer dismissal | Finished transfer detail → Dismiss Transfer |
| Empty Send illustration/CTA | Transfers section footer: “Files you send or receive appear here.” |
| Friends “You” card | Settings profile row → Profile |
| Friend cards and send glyphs | My Devices / Friends avatar rows with device badges and presence; search when count > 6 |
| Friend management sheet | Pushed FriendDetail, Friends back button, 80pt avatar/name/presence |
| Send / message / browse actions | FriendDetail grouped Send Files / Message / Browse Locations rows, using existing actions |
| Auto-accept toggle and inline rename | FriendDetail Settings group; native-style switch and Name alert |
| Presence / link probing | Check connection row with inline online/path/RTT/no-response result |
| Friend invite reveal | Share Invite row → QR and Copy Invite sheet |
| Inline remove confirmation | Destructive Remove Friend action sheet; chat-history retention copy preserved |
| Add friend modal | Add Friend sheet, Their Code field, QR scanner, Done; legacy/permanent regex dispatch preserved |
| Device linking buttons | My Devices → Link a Device; empty group explanatory footer; original link command/event flow retained |
| Profile name / picture controls | Profile → editable Name alert and tappable avatar action sheet using pickAvatar/clearAvatar |
| Personal invite code / QR | Profile → Your Code cell with QR, Copy Code and navigator.share (clipboard fallback) |
| Device identity and counts | Profile → Devices, This iPhone/device-kind value and linked-device count footer |
| Link another/new device | Profile → Link a New Device / Link This Device to Another Account |
| NameSetupModal | Non-dismissible large Welcome to DropBeam sheet, prefilled name and Continue |
| Desktop-shaped recipient chooser | Send to sheet: My Devices, Friends, Or → Quick Send (code) |
| Stacked file-source buttons | Photos and Videos / Files action group with separate Cancel |
| QR camera overlay | Large Scan QR Code sheet with camera, paste fallback and Done |
| Link progress / retry controls | Grouped Link a New Device sheet with status/error and Try Again row |
| Link-this-device QR panel | Link This Device sheet with new account-link copy, QR and Copy Code; cancellation/completion listeners preserved |

## Settings mapping (all existing mobile controls)

| Old mobile element / setting | New location |
| --- | --- |
| Devices, linked count, both link modes | Profile → Devices |
| displayName and profile avatar | Profile |
| Clear transfer cache | Transfers → confirmation ActionSheet |
| theme segmented selector | General → Appearance → System / Light / Dark checkmark rows |
| playSounds | General → Sounds |
| notifyOnComplete | General → Notifications |
| notifyOnMessage | General → Messages → Chat notifications |
| sendReadReceipts | General → Messages → Read receipts |
| giphyApiKey and explanatory text | General → Messages → Giphy Key |
| Direct peer-to-peer status | Transfers → Connection |
| Connection self-test / result | Transfers → Connection → Test connection |
| Local network permission guidance | Transfers → Connection → Local Network Access footer |
| requireDirect | Transfers → Connection → Direct connections only |
| waitForDirect, disabled when requireDirect | Transfers → Wait for a direct link; also available in Connection |
| parallelStreams | Transfers → Connection → Parallel streams |
| uploadLimitMbps | Transfers → Connection → Upload Limit; integer clamped to 0–100000 |
| showMegabits | Transfers → Connection → Speeds in megabits |
| Local / Direct / Relay / Connecting explanations | Transfers → Connection → How Transfers Connect |
| customRelay plus restart/setup guidance | Advanced → Custom relay |
| verboseLogging | Advanced → Diagnostics → Detailed logging |
| shareDiagnostics | Advanced → Diagnostics → Background diagnostics |
| diagnosticsUrl | Diagnostics → Diagnostics Endpoint, conditional on shareDiagnostics |
| Diagnostics test and HTTPS eligibility | Diagnostics → Send test; original enablement preserved |
| Export diagnostics | Diagnostics → Export logs, then native file sharing |
| labModeEnabled | Advanced → Lab Mode |
| labOperatorId and operator restrictions | Lab Mode → Operator ID |
| Copy myEid | Lab Mode → Copy This Device’s ID |
| appVer | About → Version |
| End-to-end encryption/background-transfer copy | Connection footer and Messages footer |
| preferDirectP2p (requested addition) | Transfers → Prefer direct connections |

Business persistence/rollback, sending/receiving/retry, offers, friend operations,
and account linking still use the existing store/API implementation. `useSend`
and `useSettings` are mobile presentation adapters; the old desktop views are
kept intact rather than modifying files outside this slice's ownership.

App retains its window-controls/content/overlays/toasts ErrorBoundaries,
FolderInviteModal, Toasts, keyboard handling, installNativeTabBar and DOM
MobileTabBar fallback. Chat/History/Locations remain unchanged in plain wrappers.
Native native-tab events continue switching the existing store view.

## Verification and limitations

- `npx --no-install tsc --noEmit -p .`: passed.
- `npx --no-install tsc --noEmit -p tsconfig.app.json`: passed (checks the actual app sources, beyond the root references-only config).
- `node --test tests/*.test.ts`: 66 passed, 0 failed (64 existing + 2 pure-helper tests).
- Offline Vite production build to `/tmp/dropbeam-mobile-build`: passed; bundle-size advisory remains.
- `git diff --check`: passed.
- JSX, modal nesting/dismissal, failed-receive persistence, picker resolution,
  camera cleanup, safe-area ownership and desktop branch diffs reviewed without a browser.

Items 1–6 are implemented. No browser/simulator was opened, so pixel-level iPhone
appearance and native camera/picker/share-sheet interactions need device QA.
Completed outgoing transfers can reopen the share sheet while original paths
remain in the store's existing 50-entry retry cache; unavailable paths fall back
to an explanatory transfer detail. No missing source paths are fabricated.
