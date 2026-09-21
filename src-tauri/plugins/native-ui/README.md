# Native iOS UI — phase 1 handoff

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

Chat, History and Settings are intentionally styled placeholders. QR scanning is an explicit placeholder. Recipient selection remains the existing web chooser. Native profile/onboarding, device-linking screens, chat history/composer and settings controls remain for later phases, although their requested bridge operations exist. A first installation continues using the engine's default device name until native profile controls are added.

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
