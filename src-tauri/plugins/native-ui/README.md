Beta review preparation (2026-09-21) is documented in [BETA-REVIEW.md](BETA-REVIEW.md), including validation, privacy audit, changed files and remaining device checks.

Current picker protocol supersedes the historical `sendChatFiles` flow below: Swift awaits `pickFiles`, then calls `stageChatFiles({friendId, paths})`; native snapshots are re-pushed after replies and foregrounding. Photos invokes the Rust command directly, bypassing the web chooser. Files uses `plugin:native-ui|pick_files` (Swift `pickFiles`, result `{paths: string[]}`) with security-scoped coordinated imports. Swift owns cancellable preparation state. SuperFeedback is configured once during native activation with its floating right-center trigger and Settings controls.

# Native iOS UI — phase 3

SwiftUI now owns all five tabs and their workflows. The React MobileApp remains
mounted as a hidden bridge host; its screens, onboarding, recipient chooser and
folder-invite modal stop rendering after native activation. WKWebView is never
revealed. There is no `webOverlay` protocol or legacy Rust UITabBar runtime.

## Phase 3 screens and files

All views use the unchanged `UI/Design.swift` mesh, GlassCard, GlassGroup and glass
button modifiers; iOS 26 effects retain their iOS 17 material fallbacks.

- `UI/HistoryView.swift`: searchable day-grouped recents, share/copy/remove,
  clear confirmation, recoverable storage and per-folder restore/permanent-delete.
- `UI/LocationsView.swift`: friend/presence groups and remote browser with pushed
  folders, search, next-cursor paging, multiselect, rights-aware actions, upload,
  rename/mkdir alerts, trash confirmations/results and loading/retry states.
- `UI/SettingsView.swift`: profile/avatar/invite QR, devices/linking, appearance,
  notification/sound/receipt controls, transfer preferences, upload limits,
  connection test/help, relay, diagnostics/export, Lab Mode and recovery limits.
- `UI/NativeSheets.swift`: VisionKit QR scanner with camera permission and paste
  fallback, onboarding, native Send-to sheet and inbound shared-folder invites.
- `UI/NativeFolderPicker.swift`: UIKit directory selection with security-scoped,
  coordinated import; copies the selected
  directory into `Documents/Imported Folders/<UUID>/<name>` for durable engine
  access. Rejects copying an ancestor into itself. Folder sync uses that local
  imported copy, not the external provider. Send-to waits for the system picker's
  dismissal before presenting its own sheet.
- `UI/Chat/ChatAttachment.swift`: shared `MediaViewer`, zoomable `ImageViewer`, and
  `AVPlayerViewController` wrapper used by Chat and History.
- `Bridge.swift`, `Models.swift`, `NativeUIPlugin.swift`, `RootView.swift`,
  `FriendsView.swift`: state, navigation, native presentation and entry points.
- `src/lib/nativeBridge.ts`, `nativePhase3.ts`: existing store/API adapters, JSON
  snapshots, cursor normalization, filename validation and rights checks.
- `src-tauri/src/history.rs`: iOS-only single-entry removal under the existing
  history lock/atomic writer. `ios_media.rs`: present above native sheets and
  share canonical local files within the app sandbox, including imported media.
- `src/App.tsx`, `src/mobile/MobileApp.tsx`: hidden bridge-host integration. Desktop
  screen branches are retained; store.ts and api.ts remain unchanged.
- Deleted `src-tauri/src/ios_tabbar.rs`, its module/command registration,
  `src/lib/nativeTabBar.ts`, `nativeTabBarModel.ts`, `tests/native-tabbar.test.ts`.
- `tests/native-phase3.test.ts`: five regressions for paging, revoked permissions,
  path traversal, completed history paths and same-length snapshot edits.

## Phase 3 bridge additions

All arguments/results are JSON values using the original correlated call/reply
protocol below. Void replies are null. Store actions preserve their implementation;
a consumed store error toast is propagated as a failed native action rather than
silently dismissing a sheet. Paths are local sandbox paths.

| Handler | Args | Result / existing pipeline |
| --- | --- | --- |
| `historyList` | `{}` | `HistoryEntry[]`; store.reloadHistory → api.getHistory |
| `historyClear` | `{}` | null; api.clearHistory + store reload |
| `historyRemove` | `{entryId}` | null; iOS-only remove_history_entry + reload |
| `historyOpen` | `{entryId}` | null; completed `outDir/fileNames` → api.shareFiles |
| `recoverableSummaries` | `{}` | `[{pairId,folderName,folder,bytes,itemCount,oldestMs}]` |
| `recoverableItems` | `{folder: pairId}` | `[{id,relPath,size,reason,timestampMs}]` |
| `recoverableRestore`, `recoverableForget` | `{item:{folder:pairId,id:itemId}}` | existing restoreFolderItem / forgetFolderItem result |
| `recoverableEmpty` | `{folder:pairId}` | freed bytes via clearFolderHistory |
| `recoverableEmptyAll` | `{}` | freed bytes via clearAllFolderHistory |
| `locationsList`, `locationsRefresh` | `{}` | `[{friendId,friendName,online,locations:SharedLocation[],error:string|null}]` |
| `browserList` | `{friendId,locationId,path,cursor?,query?}` | `{entries:[{name,isDir,size,modified}],hasMore,cursor:string|null,total}`; returned cursor is the **next** cursor |
| `browserDownload` | `{friendId,locationId,path,names:string[]}` | `{transferId:string|null,skipped?:string[]}` |
| `browserUpload` | `{friendId,locationId,path,source:'photos'|'files'|'folder'}` | TransferUpdate or null on picker cancel; locationsApi.upload + rememberLocationUpload + store upsert |
| `browserMkdir` | `{friendId,locationId,path,name}` | existing locations.mkdir result |
| `browserRename` | `{friendId,locationId,path,from,to}` | existing locations.rename result |
| `browserTrash` | `{friendId,locationId,path,names:string[]}` | `[{name,trashPath?:string,error?:string}]`; each item can fail independently |
| `clearTransferCache` | `{}` | bytes freed |
| `exportLogs` | `{}` | exported path; existing exportDiagnostics then native share |
| `diagnosticsTest`, `connectionTest` | `{}` | string via diagnosticsTest / irohSelftest |
| `appVersion` | `{}` | string from store/updater appVersion |
| `myDeviceInfo` | `{}` | refreshed normalized myDevice object |
| `needsName` | `{}` | bool; settings loaded AND (empty name OR missing dropbeam.namedSelf) |
| `setDisplayName` | `{name}` | null; trimmed nonempty name → saveSettings; namedSelf set only after successful persistence |
| `setAvatar` | `{path?:string}` | null; existing avatar command for an explicit path or existing mobile photo picker/store action |
| `clearAvatar` | `{}` | null; store.clearAvatar |
| `pickFiles` | `{source:'photos'|'files'}` | `[localPath]`; native picker only, no recipient selection |
| `sendToFriend` | `{friendId,paths:string[]}` | null; existing store send, then clear pendingSend |
| `quickSend` | `{paths:string[]}` | null; store.sendPaths (same code flow as the React chooser) |
| `dismissSend` | `{}` | null; clear pendingSend |
| `refreshRecipients` | `{}` | null; refresh device identity and probe peer presence |
| `acceptFolderInvite` | `{code}` | bool (false on cancel); native directory import → api.acceptPair → reloadPairs |

Existing `updateSettings({patch})`, `myInviteCode()`, `linkDeviceBegin()` (code
string), `linkDeviceCancel()` and `linkDeviceSend({code})` (normalized LinkResult)
are reused. Appearance writes `settings.theme`. Linking this device cancels on
dismiss, handles late begin replies, and closes when friends gain a matching
accountPub device, including an existing friend becoming a linked device.
Folder-invite dismissal consumes only the current queued invitation.
Add Friend and both scanned/pasted folder invites use the
same native scanner. Receiving files still uses the existing Send receive-code flow.

Additional diffed JSON state keys:

- `history`: unmodified store HistoryEntry array.
- `locations`: the locationsList shape above, with live store presence. The existing
  React Locations view owns its map locally, **not in Zustand**; the native adapter
  reads/writes the same validated `dropbeam.locations` cache and calls locationsApi.
  Only id/name/rights are cached; capacity and reachability are not persisted.
- `needsName`: bool, using the same onboarding persistence condition.
- `pendingSend`: string array, including engine/launch-file requests. Swift also
  stages freshly picked paths locally until the user chooses a recipient.

`folder-history://changed`, `locations://changed` and `folder-invite://incoming`
are forwarded unchanged; recovery/browser views refresh and inbound invites queue
natively. All existing Phase 2 state keys/events remain.

Native plugin command addition: `plugin:native-ui|pick_folder`, args `{}`, reply
`{path:string|null}`. Its Swift entry is pickFolder; generated command permissions
are included in the iOS-only native-ui default capability. This supplies the
missing iOS directory picker without changing the desktop or generic api wrapper.

Timeouts: normal calls 30s, remote browser operations / locations refresh 180s, interactive pickers /
folder import 30 minutes. Late replies cannot resolve an already-finished call.
An expired call does not cancel its underlying engine action.

## Phase 3 verification

Verified offline on 2026-09-21 after completing the interrupted implementation:

- `CARGO_TARGET_DIR=/Users/ashtonmiller/DropBeam-ios/src-tauri/target cargo check --offline --target aarch64-apple-ios-sim`: **passed**, including recompilation of the actual Swift package; 40 existing Rust warnings.
- Full plugin Swift typechecks against the built Tauri/SwiftRs modules at
  `arm64-apple-ios17.0-simulator` and `arm64-apple-ios26.0-simulator`: **passed**.
- `npx --offline tsc --noEmit -p .`: **passed**; also checked
  `-p tsconfig.app.json` because the root config contains project references.
- `node --test tests/*.test.ts`: **78 passed, 0 failed, 0 skipped**, including
  five phase-3 regressions. The three legacy tab-bar tests were removed with that module.
- `git diff --check`: **passed**. `store.ts`, `api.ts`, desktop views/components,
  `Design.swift`, package manifests/locks and version configuration are unchanged.
  Shared App integration retains the desktop branches; Rust additions are iOS-only.

The existing validation-only `/tmp/dropbeam-native-tools/swift` wrapper disables
SwiftPM sandboxing, automatic resolution and package updates. Caches are under
`~/Library/Caches`, Cargo uses the requested shared target directory, and
`GIT_ALLOW_PROTOCOL=file` prevents remote SwiftPM transport. No new dependency,
version change, commit or push was made.

Logs: `/tmp/dropbeam-p3-cargo.log`, `/tmp/dropbeam-p3-tests.log`,
`/tmp/dropbeam-p3-swift-17.0.log`, `/tmp/dropbeam-p3-swift-26.0.log`.

Runtime verification remains: a linked app launch; light/dark and accessibility
text-size review; physical-camera scanning; Files/provider import, avatar picking,
media playback/share and sheet transitions; real peer device linking, remote file
operations and transfer completion. These compile/test results do not claim that
device or simulator interaction testing occurred. All requested phase-3 workflows
are implemented; folder sync intentionally uses its imported sandbox copy.

---

# Historical phase 2 and phase 1 handoff (superseded where noted above)

# Native iOS UI — phase 2 handoff

## Phase 2: native Chat

Chat now uses a native conversation list, friend picker, pushed thread, keyboard-safe
composer, reply/edit banners, staged attachments, Tapback context menus, local search,
typing, unread badges, file transfer progress/retry, image zoom/share and AVPlayer.
iOS 26 glass is availability-guarded with material fallbacks on iOS 17. The Send,
Friends and placeholder tab roots remain scrolling views directly inside their
NavigationStacks, with an additional 24pt bottom scroll-content margin. Only the
background ignores safe areas. Receive-code placeholders use the proportional body
font; entered codes use monospaced body text.

### Chat handler additions / changes

All handlers use the existing store actions/API. Void replies are JSON null.

| Handler | Args | Result / behavior |
| --- | --- | --- |
| `chatThread` | `{ friendId: string }` | `ChatMessage[]`; opens when necessary, loads overview, returns store history; subsequent pushes remain authoritative |
| `openChat` | `{ friendId: string }` | Store openChat sets activeChatId and engine active thread; `chatOpen` drives native navigation |
| `closeChat` | `{ friendId?: string }` | Store closeChat; optional ID prevents an old destination closing a new peer |
| `sendChatText` | `{ friendId, text, replyTo?: string }` | Reply ID resolved against store messages; store sendChat, or shareFilesInChat with the staged paths and caption |
| `sendChatFiles` | `{ friendId, source: 'photos' \| 'files' }` | Native picker → store stageChatFiles; attachments are sent with the next composer send, just like the web composer; discards late picker results after switching peers |
| `removeChatDraftFile` | `{ path: string }` | Store unstageChatFile |
| `reactToMessage` | `{ friendId, messageId, emoji }` | Store reaction toggle |
| `editMessage` | `{ friendId, messageId, text }` | Store editChatMessage |
| `deleteMessage` | `{ friendId, messageId }` | Store deleteChatMessage (unsend) |
| `markChatRead` | `{ friendId }` | Store markChatRead; the list action briefly opens/closes through the store without pushing a native destination |
| `setTyping` | `{ friendId, bool: boolean }` | Existing api.sendTyping; first keystroke, heartbeat while typing, off after 3 seconds idle / send / leave |
| `retryChatFile` | `{ friendId, messageId }` | Resolve fileXferId and call store resendChatFile; received failures ask the sender to retry |
| `openChatFile` | `{ path: string }` | Existing native api.shareFiles, including system preview/save/share actions |
| `nativeChatFocus` | `{ bool: boolean }` | Native scene drives store windowFocused and existing receipt gates; hidden DOM focus cannot override it |
| `chatGifs` | `{ query: string }` | Existing Giphy provider → `GifResult[]`; only enabled with a settings key |
| `sendChatGif` | `{ friendId, id: string }` | Existing store sendGif using a previously returned result |

There is no delete-for-me or delete-conversation store/API operation. Neither is
shown or implemented locally. File messages use the existing caption send path,
which does not support quoted attachments; selecting an attachment clears Reply.

### Chat snapshots and events

All snapshots are diffed by serialized JSON, including edits/reactions/receipts:

- `chatOverview`: existing `ChatOverview[]` with additive `unread`.
- `chatUnread`: `{ [friendId: string]: number }`; its nonnegative sum is the tab badge.
- `chatTyping`: `{ [friendId: string]: boolean }`.
- `thread`: `{ friendId: string, messages: ChatMessage[] } | null`; only the active
  thread is pushed. Swift caches threads by peer and never overwrites a newer push
  with an older request reply.
- `chatDraftFiles`: `string[]` of staged local paths.
- `transfers`: existing transfer array, with additive `chatOnly: boolean`.
  Chat batch entries use the shared `fileXferId` as `id`; they include existing
  `chatTransfer.completedPaths` and completed `sharePaths`. Swift joins these to
  messages; Send filters out chat-only entries. Existing restoredChatTransfer
  supplies durable outcomes when a live batch is absent. A chat-note delivery
  receipt does not override an explicit failed transfer.
- `chatOpen` event: `{ friendId: string | null }` drives push/pop, including opens
  from Friends or an existing notification action.
- `chat://message` remains forwarded unchanged. Transfer progress comes through
  the existing diffed transfers snapshot; no second transfer event implementation.

The Swift ChatMessage has only four required fields: `id: String`, `peerId: String`,
`fromMe: Bool`, `ts: Double` (milliseconds). Optional fields mirror the API:
`kind`, `text`, `files: [String]`, `bytes`, `path`, `status`, `seq`, `replyTo`,
`replyPreview`, `reactions: [{ emoji, fromMe }]`, `edited`, `deleted`,
`gif: { url, w, h }`, `fileXferId`, `fileXferFailed`. Unknown fields/status values
are tolerated. Received media previews use local files only.

### Phase 2 files and verification

- `src/lib/nativeBridge.ts`: handlers, focus/navigation coordination and snapshots.
- `src/lib/nativeChatBridge.ts`: pure reply/source/thread/transfer adapters.
- `tests/native-chat-bridge.test.ts`: five regression tests for those adapters,
  including reply validation, peer isolation, shared transfer IDs and message diffs.
- `ios/Sources/NativeUIPlugin/Bridge.swift`, `Models.swift`: native protocol models,
  subscriptions, navigation and calls.
- `ios/Sources/NativeUIPlugin/UI/Chat/ChatsView.swift`: list, friend picker, glass and dates.
- `ios/Sources/NativeUIPlugin/UI/Chat/ConversationView.swift`: thread, grouping,
  scrolling, search, receipts and typing indicator.
- `ios/Sources/NativeUIPlugin/UI/Chat/ChatBubble.swift`: bubble rendering, quotes,
  highlights, Tapbacks and context menus.
- `ios/Sources/NativeUIPlugin/UI/Chat/ChatComposer.swift`: typing, staging,
  reply/edit, send and native GIF picker (using the existing provider).
- `ios/Sources/NativeUIPlugin/UI/Chat/ChatAttachment.swift`: transfer joins,
  local thumbnails, retry, multiple-file chooser, zoom, AVPlayer and share.
- `UI/RootView.swift`, `SendView.swift`, `FriendsView.swift`: tab integration,
  scene lifecycle and scrolling/placeholder fixes.

Offline checks: simulator-target Cargo check (including the actual Swift package),
full-source Swift typechecks with the built Tauri modules at iOS 17 and iOS 26,
TypeScript root and app configs, and 76 Node tests (71 existing + 5 new).
The pre-existing validation-only `/tmp/dropbeam-native-tools/swift` wrapper disables
SwiftPM package resolution/updates; Git allows only local-file transport. Cargo
emits the existing 40 Rust warnings. No new dependency, commit, push, version change,
store/API implementation change, or React MobileApp edit.

The orchestrator still needs to build/run the iPhone 17 Pro simulator: review tab
insets on long Send/Friends/Chat lists, iOS 26 glass and context-menu layout,
keyboard/picker transitions, pinch zoom/video/share, light/dark and accessibility
text sizes, plus real peer receipts/typing/progress/retry. Network-dependent GIF
search/send was not exercised during this offline work. This handoff does not
claim simulator visual QA or a linked application build.

## Phase 1 shell reference

SwiftUI owns the iOS screen. The existing React MobileApp, Zustand store, Tauri commands and Rust transfer engine remain alive underneath it. No existing app/package versions were changed, and no commit or push was made. No new registry dependency was added; the new local plugin uses dependencies already in the Cargo lock/cache. Desktop execution stays behind the existing MOBILE_UI and iOS target gates.

## Installation and lifecycle

App starts the bridge only for MOBILE_UI + Tauri. The bridge marks the native shell active **before** invoking `plugin:native-ui|activate`, preventing `installNativeTabBar()` from installing the Rust UIKit bar. If activation fails, the flag resets and existing fallback navigation can install.

Swift activation runs on the main thread, gets `manager.viewController`, adds one `UIHostingController` as a child, constrains all four edges to the root view, and injects `Bridge.shared`. The background ignores safe areas; controls retain system safe-area behavior. WKWebView stays attached with alpha 0, interaction disabled and accessibility hidden. Repeated activation reuses the hosting controller. SwiftUI TabView supplies the system tab bar; iOS 26 gets Liquid Glass, with material and bordered-button fallbacks on iOS 17.

The existing web recipient chooser temporarily owns the screen through `webOverlay` events: the host is hidden and the webview is made visible/interactive while `pendingSend` is nonempty. Closing or choosing a recipient restores SwiftUI. The hidden React onboarding is suppressed while the native shell is active so it cannot obscure that chooser. No onboarding completion preference is written.

Tauri's plugin builder places its copied API in the crate's `.tauri/tauri-api`. The checked-in `ios/.tauri -> ../.tauri` symlink lets Package.swift use the requested `./.tauri/tauri-api` dependency. The copied API and build products are ignored. Package.resolved pins the already-cached SwiftRs 1.0.7 dependency inherited from Tauri.

## Exact bridge protocol

All values are JSON values, not JSON encoded inside strings. Unknown commands return a failed reply. Void results become null. Unknown snapshot fields are ignored; missing optional model fields decode as nil. Invalid snapshots are rejected without crashing or replacing the last valid snapshot.

```ts
// Swift -> WKWebView; id is a monotonically increasing integer starting at 1.
window.__dbBridge.call(id, name, args)
// Implemented with a JSON-serialized [id, name, args] spread into call, so user
// text is never interpolated as JavaScript source. The JS Promise is not awaited
// by evaluateJavaScript; replies arrive through the separate Tauri command.

// JS -> Rust -> Swift; activate/reply/state/event resolve with null.
invoke('plugin:native-ui|activate', {})
invoke('plugin:native-ui|reply', { id, ok: true, value: result ?? null })
invoke('plugin:native-ui|reply', { id, ok: false, value: errorMessage })
invoke('plugin:native-ui|state', { key, value })
invoke('plugin:native-ui|event', { name, payload })
```

Swift keeps checked continuations indexed by id, removes them on reply/error, cancels each deadline on completion, and times out after 20 seconds. Late/duplicate replies are ignored. A timeout does not cancel the underlying engine operation or native picker; check its status before repeating it.

State keys, pushed once after activation and thereafter only when serialized JSON changes:

- `friends`: Friend array, excluding pairing secrets; camelCase keys.
- `myDevice`: null or `{ name, endpointId, deviceKind, accountPub, linkedDevices }`, normalized from the API's snake_case fields. Refreshed at startup and `friends://changed`.
- `transfers`: newest-first TransferUpdate array from the store order, with an additive `sharePaths: string[]` from the existing retry/destination path logic. Empty canceled transfers are omitted as in the mobile Send view.
- `settings`: Settings object or null.
- `chatOverview`: ChatOverview array with additive `unread` from `chatUnread` (the engine's `count` is not an unread count).
- `presence`: `{ [friendId]: boolean }`, using `friendOnlineState(friend.name, friendSeen, folderStatuses)`. A 15-second resnapshot timer allows presence to expire without another store mutation.

State/event delivery is serialized. Tauri events `chat://message` and `friend://presence` are forwarded with their original JSON payloads and posted to native NotificationCenter under `DropBeam.<name>`. Published snapshots remain authoritative. Internal events:

- `webOverlay`, `{ visible: boolean }`: temporarily reveal/restore the web chooser.
- `view`, `{ name: string }`: reflect store navigation back into native selection.
- `error`, `{ message: string }`: surface existing store error toasts in a native alert.

Handler names and argument objects:

| Handler | Args | Implementation/result |
| --- | --- | --- |
| `pickAndSend` | `{ source: 'photos' \| 'files', friendId?: string }` | Shared mobile picker, then existing send action or pending-send chooser |
| `sendToFriend` | `{ friendId, paths: string[] }` | Store send action |
| `receiveWithCode` | `{ code }` | Store receive; false result becomes a failed reply |
| `cancelTransfer` | `{ id }` | Existing API command |
| `retryTransfer` | `{ id }` | Store send retry, or receive again using the receive code |
| `openChat` | `{ friendId }` | Store openChat; native selection changes to Chat |
| `sendChatText` | `{ friendId, text }` | Store sendChat |
| `setView` | `{ name }` | Store setView; allowed native tab names only |
| `pingFriend` | `{ id }` | Store ping + probe; `{ online, path: string \| null, rttMs: number \| null }` |
| `removeFriend` | `{ id }` | Store removeFriend |
| `renameFriend` | `{ id, name }` | Store renameFriend |
| `setAutoAccept` | `{ id, bool: boolean }` | Store setFriendAutoAccept |
| `myInviteCode` | `{}` | API; returns string |
| `addFriendByCode` | `{ code }` | Store permanent-code pairing |
| `acceptFriend` | `{ code }` | Store invite-code pairing |
| `shareFiles` | `{ paths: string[] }` | Existing native share API |
| `linkDeviceBegin` | `{}` | API; returns code string |
| `linkDeviceCancel` | `{}` | API |
| `linkDeviceSend` | `{ code }` | API + friend refresh; `{ endpointId, name, deviceKind }` |
| `updateSettings` | `{ patch: Partial<Settings> }` | Store saveSettings |
| `respondToOffer` | `{ id, accept: boolean }` | Additional handler so incoming transfers can be accepted/declined natively |

Store actions retain their existing return and error semantics; actions which already consume exceptions and show toasts also surface those toasts through the native error event.

## Validation

- `cargo check --offline --target aarch64-apple-ios-sim`: **passed**, including SwiftPM compilation of the actual NativeUIPlugin/Tauri package and Rust plugin. Existing app code emits 40 warnings; no Rust engine changes were made.
- `xcrun -sdk iphonesimulator swiftc -typecheck ... -target arm64-apple-ios26.0-simulator`: **passed** for all plugin Swift sources, including NativeUIPlugin.swift, using actual built Tauri/SwiftRs modules.
- Same complete Swift typecheck targeting `arm64-apple-ios17.0-simulator`: **passed**, confirming availability guards.
- `npx --offline tsc --noEmit -p .`: **passed**. Also ran `-p tsconfig.app.json`, because the root config contains project references and no files.
- `node --test tests/*.test.ts`: **71 passed, 0 failed**, including five new protocol regression tests. The pre-existing untracked `tests/mobile-helpers.test.ts` was left untouched.
- XcodeGen regenerated the project with iOS 17.0. Original Info.plist was preserved after generation so existing version values and local-network text remain unchanged.
- `git diff --check`: **passed**.

This workspace sandbox initially blocked SwiftPM's nested sandbox and default `~/.cache/clang` writes. For the successful Cargo check, a temporary PATH wrapper invoked Swift build with `--disable-sandbox --skip-update --disable-automatic-resolution`; Swift/Clang caches were directed into `~/Library/Caches`. Git protocol settings disabled remote transport and allowed only local file transport. SwiftRs came from the existing cache. These validation-only wrappers live in `/tmp`, not in the repository. The plugin build script supplies the iOS 17 deployment default for direct Cargo invocations where Xcode has not set it.

Swift typechecks used `-module-cache-path ~/Library/Caches/dropbeam-swift` and `-I` pointing to the generated plugin's `.../release/Modules` directory. The installed SDK was iPhoneSimulator26.5.sdk. No internet access was used.

## Remaining phase work / orchestrator verification

No full application link, simulator launch, screenshot review, native picker/chooser interaction, or device-to-device transfer was performed here. The orchestrator must run the actual simulator app to verify glass rendering, controller presentation, safe areas, rotation, light/dark appearance and Dynamic Type. Compilation alone does not establish those runtime results.

History and Settings remain styled placeholders. Chat is implemented in phase 2 above. QR scanning is an explicit placeholder. Recipient selection remains the existing web chooser. Native profile/onboarding, device-linking screens and settings controls remain for later phases, although their requested bridge operations exist. A first installation continues using the engine's default device name until native profile controls are added.

Suggested simulator checks: cold launch and repeated bridge activation (one glass tab bar); search/add/rename/remove/auto-accept/check a friend; Photos and Files sends with and without a target friend; cancel the chooser and restore native navigation; receive by code and accept an incoming offer; cancel/retry/share completed transfers; switch tabs and verify unread updates; review light/dark and accessibility text sizes.

## File manifest (repository paths and line references)

| File:line | Change |
| --- | --- |
| [src-tauri/Cargo.toml:121](../../Cargo.toml) | iOS-only path dependency |
| [src-tauri/Cargo.lock:6268](../../Cargo.lock) | Local plugin lock entry; existing dependency versions unchanged |
| [src-tauri/src/lib.rs:290](../../src/lib.rs) | iOS-only plugin registration |
| [src-tauri/capabilities/ios-native-ui.json:1](../../capabilities/ios-native-ui.json) | iOS-only default permission |
| [src-tauri/gen/apple/project.yml:5](../../gen/apple/project.yml) | iOS 17 deployment |
| [src-tauri/gen/apple/app.xcodeproj/project.pbxproj:432](../../gen/apple/app.xcodeproj/project.pbxproj) | Regenerated deployment settings (also line 526) |
| [src/App.tsx:40](../../../src/App.tsx) | Start bridge, retain MobileApp, suppress hidden web onboarding |
| [src/lib/nativeTabBar.ts:14](../../../src/lib/nativeTabBar.ts) | Skip legacy native tab bar |
| [src/lib/nativeShell.ts:1](../../../src/lib/nativeShell.ts) | Early activation flag |
| [src/lib/nativeBridge.ts:1](../../../src/lib/nativeBridge.ts) | Handlers, startup, subscriptions and state/event pushes |
| [src/lib/nativeBridgeProtocol.ts:1](../../../src/lib/nativeBridgeProtocol.ts) | JSON reply and snapshot comparison boundary |
| [src/lib/mobilePick.ts:1](../../../src/lib/mobilePick.ts) | Shared picker/send and share-path logic |
| [src/mobile/useSend.ts:1](../../../src/mobile/useSend.ts) | Use shared mobile picker |
| [src/mobile/SendScreen.tsx:10](../../../src/mobile/SendScreen.tsx) | Reuse extracted share paths |
| [tests/native-bridge.test.ts:1](../../../tests/native-bridge.test.ts) | Five protocol regression tests |
| [src-tauri/plugins/native-ui/.gitignore:1](.gitignore) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/Cargo.toml:1](Cargo.toml) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/build.rs:1](build.rs) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/Package.resolved:1](ios/Package.resolved) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/Package.swift:1](ios/Package.swift) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/Bridge.swift:1](ios/Sources/NativeUIPlugin/Bridge.swift) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/Models.swift:1](ios/Sources/NativeUIPlugin/Models.swift) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/NativeUIPlugin.swift:1](ios/Sources/NativeUIPlugin/NativeUIPlugin.swift) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/UI/Design.swift:1](ios/Sources/NativeUIPlugin/UI/Design.swift) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/UI/Formatters.swift:1](ios/Sources/NativeUIPlugin/UI/Formatters.swift) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/UI/FriendsView.swift:1](ios/Sources/NativeUIPlugin/UI/FriendsView.swift) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/UI/RootView.swift:1](ios/Sources/NativeUIPlugin/UI/RootView.swift) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/UI/SendView.swift:1](ios/Sources/NativeUIPlugin/UI/SendView.swift) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/permissions/autogenerated/commands/activate.toml:1](permissions/autogenerated/commands/activate.toml) | Generated by tauri_plugin::Builder |
| [src-tauri/plugins/native-ui/permissions/autogenerated/commands/event.toml:1](permissions/autogenerated/commands/event.toml) | Generated by tauri_plugin::Builder |
| [src-tauri/plugins/native-ui/permissions/autogenerated/commands/reply.toml:1](permissions/autogenerated/commands/reply.toml) | Generated by tauri_plugin::Builder |
| [src-tauri/plugins/native-ui/permissions/autogenerated/commands/state.toml:1](permissions/autogenerated/commands/state.toml) | Generated by tauri_plugin::Builder |
| [src-tauri/plugins/native-ui/permissions/autogenerated/reference.md:1](permissions/autogenerated/reference.md) | Generated by tauri_plugin::Builder |
| [src-tauri/plugins/native-ui/permissions/default.toml:1](permissions/default.toml) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/permissions/schemas/schema.json:1](permissions/schemas/schema.json) | Generated by tauri_plugin::Builder |
| [src-tauri/plugins/native-ui/src/lib.rs:1](src/lib.rs) | Plugin source/package/permissions |
| [src-tauri/plugins/native-ui/ios/.tauri:1](ios/.tauri) | Symlink to ../.tauri (not a copied vendored API) |
