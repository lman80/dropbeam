# DropBeam on an iPhone

Use this checkout (`~/DropBeam-ios`, branch `ios`). Install Xcode with its iOS
platform support, Node/npm, and Rust's device target:

```sh
cd ~/DropBeam-ios
npm ci
rustup target add aarch64-apple-ios
npm run build
open src-tauri/gen/apple/app.xcodeproj
```

## Run from Xcode

1. In Xcode → Settings → Accounts, sign in with your Apple ID. A free Apple ID
   works for personal device testing; paid membership is not required.
2. Select the **app** project, **app_iOS** target, then **Signing & Capabilities**.
   Enable **Automatically manage signing** and select your **Personal Team**.
   If `com.dropbeam.app` cannot be registered to your team, use a unique personal
   identifier and keep the Tauri identifier and Xcode bundle identifier aligned.
3. Connect and unlock the iPhone, accept **Trust This Computer**, and select it
   as the run destination for the **app_iOS** scheme. Enable Developer Mode under
   Settings → Privacy & Security if iOS requests it, then restart as prompted.
4. Keep a Tauri CLI session running for the generated **Build Rust Code** phase.
   For development, run `npx tauri ios dev --open --host` from this checkout and
   leave the terminal open. This opens the same Xcode project; the phone must
   be able to reach the Mac's development server. Then press **Run** (⌘R).
5. If the phone reports an untrusted developer, open **Settings → General →
   VPN & Device Management**, select your developer profile and trust it, then
   run DropBeam again. The profile appears after a signed app is installed.
6. Allow Local Network access when prompted so iroh can discover nearby peers.

Free-team provisioning profiles expire **7 days after issuance**. Rebuild and
reinstall from Xcode to renew the app; it can stop opening after expiry.
See [Apple's account overview](https://developer.apple.com/help/account/basics/about-your-developer-account).

## CLI alternative: signed device build and installation

After configuring your team and device provisioning in Xcode, use your actual
team ID and the device identifier reported by `devicectl`:

```sh
cd ~/DropBeam-ios
npm run build
APPLE_DEVELOPMENT_TEAM=YOUR_TEAM_ID npx tauri ios build --debug --target aarch64 --archive-only
xcrun devicectl list devices
xcrun devicectl device install app --device YOUR_DEVICE_ID src-tauri/gen/apple/build/app_iOS.xcarchive/Products/Applications/DropBeam.app
xcrun devicectl device process launch --device YOUR_DEVICE_ID com.dropbeam.app
```

Use your personal bundle identifier in the last command if you changed it.
`--archive-only` avoids IPA export; the signed device `.app` is inside the
archive above. Do not use `--no-sign` for an iPhone. If automatic provisioning
needs interaction or an initial device registration, finish that in Xcode and
Run there. To export a development IPA instead, use:

```sh
APPLE_DEVELOPMENT_TEAM=YOUR_TEAM_ID npx tauri ios build --debug --target aarch64 --export-method debugging
```

The CLI reports the output path under `src-tauri/gen/apple/build/arm64`.
You can also install a signed build through Xcode → Window → Devices and
Simulators → your iPhone → Installed Apps. Device signing/install commands
above are instructions for the operator and were not executed in this session.
[Tauri signing reference](https://v2.tauri.app/distribute/sign/ios/) and
[team environment variable](https://v2.tauri.app/reference/environment-variables/).

## Simulator build

```sh
rustup target add aarch64-apple-ios-sim
npm run build
npx tauri ios build --debug --target aarch64-sim --no-sign --ci
```

Output: `src-tauri/gen/apple/build/arm64-sim/DropBeam.app`. This is for an Apple
Silicon simulator, not a physical iPhone. If Tauri reports `Directory not empty`
while renaming the app, move the existing `build/arm64-sim` directory to an unused
backup name before retrying. No custom DerivedData setting was needed here.
See [IOS-BUILD.md](IOS-BUILD.md) for the verified build, timestamps and warnings.

## Current iOS limits

These reflect this branch's Rust `cfg(desktop)` / `cfg(mobile)` gates and
frontend `MOBILE_UI` checks, not a claim that every desktop subsystem is removed.

| Feature | Current behavior / evidence |
| --- | --- |
| Desktop drag & drop | Use the file picker. `DropZone.tsx` changes the phone UI to “Choose files”; the shared `onFileDrop` listener in `src/lib/api.ts` still exists, so it is not fully cfg-gated away. Native inter-app iOS dragging is not implemented here. |
| Shared folders / total-sync | No phone folder tab (`MobileTabBar.tsx`); `commands.rs::pick_directory` returns `None` on mobile. The sync manager still compiles and starts in `lib.rs`; arbitrary desktop folders and reliable suspended syncing are unsupported. |
| Tray / menu-bar popover | Tray construction and helpers in `lib.rs` are desktop-only; the macOS panel dependency is macOS-only. |
| Autostart | Plugin dependency and registration are desktop-only (`Cargo.toml`, `lib.rs`); mobile ignores the startup preference. |
| In-app updater / relaunch | Updater and process plugins are desktop-only. `store.ts` skips automatic update checks on mobile; `SettingsView.tsx` explains reinstalling a new build. |
| Transfers while suspended | No iOS background transfer service or background task integration is configured. Keep DropBeam foregrounded during transfers; the live iroh endpoint does not grant background execution. |
| Desktop Trash | `sync.rs::delete_local` only uses the Trash crate on desktop; the mobile fallback removes the file directly. |

The initial default name uses `UIDevice.currentDevice.name`, falling back to
“My iPhone” if unavailable. iOS 16+ normally returns a generic “iPhone”/“iPad”
without Apple's user-assigned-name entitlement. Existing saved names (including
“Unknown”) are preserved; edit the name in Settings if needed.
[Apple UIDevice naming behavior](https://developer.apple.com/documentation/uikit/uidevice/name).

The viewport covers the screen, with theme backgrounds behind the safe areas;
header and tab controls are inset around the notch and home indicator. The plist
uses view-controller status-bar appearance and the default native style. Check
status-bar contrast with both system appearance and a manually overridden app
theme during operator QA; no native per-theme status-bar bridge was added.
