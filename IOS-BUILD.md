# Mobile keyboard viewport fix — 2026-09-09

MOBILE_UI now locks the document body in place and sizes/anchors the app root
using VisualViewport resize and scroll events (height and offsetTop), with a
window resize fallback. The flex layout reserves the header, composer and tabs;
the message list owns scrolling. Composer focus jumps to the latest message,
programmatic focus uses preventScroll, and a ResizeObserver keeps the bottom
anchored after opening/layout changes while respecting readers who scroll up.
Mobile textarea text is 16px to avoid focus zoom.

Verified:
- `npm run build` passed.
- `npx tauri ios build --debug --target aarch64-sim --no-sign --ci` passed
  outside the sandbox after CoreSimulator access failed inside it.
- Browser mock backend at 402x874, `?mobile=1`, 60-message thread:
  open and focus land at bottom (within 0.5px); scrollY remains zero.
  VisualViewport height 550 / offsetTop 150 simulation keeps header at visible
  y=0 and conversation header at y=46; composer bottom moves from 811.5 to
  487.5, above the viewport bottom. Dismissal restores the original bounds.
  A 44px accessory-bar shrink preserves a reader's scrollTop=300; subsequent
  focus returns to bottom. Mock send clears composer with header/scroll stable.
  Desktop at 1200x874 has no mobile class, fixed body, or viewport override.
- Installed and launched on iPhone 17 Pro (iOS 26.5),
  E0643E29-45EB-4EFA-B1D1-5EA9565A9D38. Screenshot confirms app launch.
  Native Chat taps repeatedly fail with CUA `noWindowsAvailable`, so native
  keyboard/accessory-bar interaction remains unverified.

Fresh bundle:
`/Users/ashtonmiller/DropBeam-ios/src-tauri/gen/apple/build/arm64-sim/DropBeam.app`

Bundle mtime: **2026-09-09 03:00:45.736998 +08:00**.
Executable mtime: **2026-09-09 03:00:42.559642 +08:00**.

Previous simulator output preserved in `arm64-sim-before-keyboard-*`.
Existing chunk-size, bundle-ID, Xcode destination and blake3 SDK warnings remain.
Changed files: src/App.tsx, src/index.css, src/views/ChatView.tsx, IOS-BUILD.md.
No subagents or push; source work restricted to this worktree.

---

# Mobile Chat and 402pt layout verification — 2026-09-09

Source commits: `9262a68`, `c7f1a83`, `f665ea5`, `de50dee` (branch `ios`).

- MOBILE_UI Chat starts with the full-width thread list. Selecting a thread, the
  Friends message icon, or a chat notification opens the full-width conversation.
  Back clears the active thread. Header retains friend name/presence; the composer,
  attachment and emoji buttons stay above the tab bar. Mobile timestamps stay
  inside the message area; missing friend records do not block a deep link.
- Shared CSS under `html.mobile` lets card actions wrap, constrains inputs and
  allows Settings controls to move below their labels. The iOS download folder
  was already read-only and remains so. Desktop Chat still has a 232px list and
  an adjacent conversation (750px at a 1200px browser viewport).
- iOS chat notifications now carry chatPeerId. A vendored notification plugin
  preserves this metadata, safely reads notifications created before a restart,
  and queues taps until the JS channel is ready. Only iOS grants the new command.
  Upstream desktop/Android implementations are unchanged; patch details are in
  src-tauri/vendor/tauri-plugin-notification/DROPBEAM-PATCH.md.

Verified commands:

```sh
npm run build
cargo check --offline --manifest-path src-tauri/Cargo.toml --target aarch64-apple-ios-sim --lib
cargo test --offline --manifest-path src-tauri/Cargo.toml
npx tauri ios build --debug --target aarch64-sim --no-sign --ci
```

All passed. Rust: **150 passed, 0 failed, 6 ignored**. The sandbox run failed in
23 loopback network-monitor tests; the unrestricted offline run passed. SwiftPM
and Xcode also required execution outside the sandbox, as in previous builds.
Previous simulator outputs were preserved in timestamped `arm64-sim-before-*`
directories. No push or subagents; no files in the ~/DropBeam checkout changed
(Git necessarily writes the ios worktree's shared metadata under its .git).

Fresh bundle:
`/Users/ashtonmiller/DropBeam-ios/src-tauri/gen/apple/build/arm64-sim/DropBeam.app`

Bundle mtime: **2026-09-09 02:50:30.608045 +08:00**.
Executable mtime: **2026-09-09 02:50:26.204327 +08:00**.
Installed and launched on iPhone 17 Pro, iOS 26.5
(`E0643E29-45EB-4EFA-B1D1-5EA9565A9D38`). The final app log confirms
`Chat notification tap listener ready`.

Browser interaction/DOM QA at `?mobile=1`, 402x874, mock backend:

- Chat list/conversation, Back, Friends message shortcut, emoji picker and a
  deep link without any loaded friend record passed. Conversation/composer span
  x=0…402. At 402x400, composer y=276.5…337.5 directly precedes the tab bar.
- Friends, long friend names, removal confirmation controls, own QR, friend
  invite/QR, receive-with-code, History Recents and Recoverable files all passed
  horizontal bounds checks. Every rendered Settings button/input can be scrolled
  fully into the main viewport.
- Populated waiting/code/QR, transferring, completed, failed and manual-accept
  transfer cards passed with long filenames, long codes and long error text.
- Notification routing selects tap payloads and ignores dismissals/unrelated
  notifications. Native channel initialization is verified; actual notification
  taps, cold-start delivery and iOS keyboard behavior remain runtime QA items.

Simulator screenshots work and confirm the new app launches, but repeated CUA
`noWindowsAvailable` errors prevent tapping its controls. Browser screenshot
capture also timed out, so browser verification used interaction and DOM bounds.
No physical-device or end-to-end transfer test was performed. Existing frontend
chunk-size, Xcode destination and blake3 SDK/deployment-target warnings remain.

Changed application files: src/views/ChatView.tsx, src/views/SettingsView.tsx,
src/index.css, src/store.ts, src/lib/chatNotifications.ts,
src-tauri/src/iroh_net.rs, src-tauri/Cargo.toml, src-tauri/Cargo.lock,
src-tauri/capabilities/ios-notifications.json, the vendored notification plugin,
and this build log.

---

# Native Photos, Files visibility, and Share — 2026-09-09

Implementation commits: `00b3fcf`, `cdfbde2`, `a3b89f1`.

Verified final commands (from this worktree):

```sh
npm run build
cargo check --offline --manifest-path src-tauri/Cargo.toml --target aarch64-apple-ios --lib
npx tauri ios build --debug --target aarch64-sim --no-sign --ci
cargo test --offline --manifest-path src-tauri/Cargo.toml
```

All passed. Rust: **150 passed, 0 failed, 6 ignored**. The sandbox run
failed in the same 23 loopback network-monitor tests documented below; the
unrestricted offline run passed. Xcode also required unrestricted execution.
Existing simulator build outputs were preserved under timestamped
`arm64-sim-before-media-*` directories before rebuilding.

Final bundle: `src-tauri/gen/apple/build/arm64-sim/DropBeam.app`.
Bundle mtime: **2026-09-09 02:18:26.127937 +08:00**.
Executable mtime: **2026-09-09 02:18:22.156729 +08:00**.
Installed and launched on the booted iPhone 17 Pro (iOS 26.5).

Changes:

- Mobile Send has Photos and Files buttons. Photos invokes an iOS-only Rust
  PHPickerViewController command with unlimited multi-selection and image/video
  filters. NSItemProvider results are copied into unique app temp directories
  before provider callbacks return. Those paths feed the existing send chooser.
  Files retains the existing document-picker command. Desktop UI is unchanged.
- Receive defaults/fallbacks use app Documents on iOS. Startup refreshes and
  persists the path after container changes. Verified the installed app's
  settings.json points to its current container's Documents directory and that
  this directory exists. Both UIFileSharingEnabled and
  LSSupportsOpeningDocumentsInPlace were already true in source Info.plist and
  project.yml; verified both remain true in the final built plist.
- Mobile received cards and History rows have Share, passing the received file
  URLs to UIActivityViewController. Native presentation runs on the main thread,
  includes an iPad popover anchor, and exposes the standard system activities.
  The existing photo-library add usage description remains in the built plist.

Simulator smoke test: Photos opened the native photo library; selecting two
sample photos produced “Send 2 files to…” and readable copied JPEGs (1,484,524
and 4,127,524 bytes). No transfer was sent. This interaction was tested before
`a3b89f1` refined the requested representation to public.image/public.movie;
that final refinement passed both iOS compilation and the simulator rebuild.

Remaining runtime QA: document-picker button, actual video/iCloud imports,
Share on both received cards and History, Save Image/Save Video in the system
sheet, iPad popover layout, and Files-app browsing. Repeated computer-control
`noWindowsAvailable` errors prevented completing those interactions. A temporary
local received-file fixture was removed. No separate Save to Photos button was
added; use the system sheet's supported save activity. No physical-device or
end-to-end receive smoke test was performed.

Nonfatal warnings remain: frontend chunk size, existing Rust warnings, bundle
identifier suffix, multiple Xcode destinations, and blake3 simulator SDK versus
iOS 14 deployment target. Rustfmt was unavailable in the installed toolchain.
No subagents or push; source changes stayed in this worktree.

---

# Simulator dialog/code verification — 2026-09-09

Source commits: `5c1559e` (Rust code prefixes) and `326ac7b` (mobile UI).

Verified commands:

```sh
npm run build
npx tauri ios build --debug --target aarch64-sim --no-sign --ci
cd src-tauri && cargo test --offline
```

All passed. Rust: **150 passed, 0 failed, 6 ignored**. The sandbox initially
blocked network-monitor creation in 23 loopback tests; the full suite passed
outside the sandbox. New tests cover keyboard capitalization, surrounding
whitespace/newlines, invalid prefixes/payloads, and byte-preserved payloads.

Fresh bundle: `src-tauri/gen/apple/build/arm64-sim/DropBeam.app`.
Bundle mtime: **2026-09-09 02:01:04.731 +08:00**.
Executable mtime: **2026-09-09 02:01:02.522 +08:00**.
Executable SHA-256: `b901ed80d3d8f732bfab929e210ce1603ec0cc0284b70c15fbe94b1283027f6a`.

Xcode required execution outside the sandbox. Packaging encountered the known
existing-output collision; the previous `arm64-sim` output was preserved as
`arm64-sim-before-dialog-fixes`, then the same build command succeeded.
Existing chunk-size, destination, bundle-ID and simulator SDK warnings remain.

Browser QA (`?mobile=1`, mock backend): at 402px width, Add friend spans
x=16…386 (370px), with no horizontal overflow. At 300px viewport height,
the dialog is 268px tall and scrolls internally (378px scroll content).
Invalid input and an injected backend rejection both produce a visible inline
alert. Capitalized/whitespace-wrapped codes reach the mocked add operation.
Desktop styling remains 440px wide and the inline error is visible there too.

Audit: all six fixed app overlays share the mobile sizing contract (name setup,
Add friend, folder pairing, incoming folder invite, invite QR, recipient chooser).
Friend rename/QR and receive-code controls are inline; settings uses inline
sections/native dialogs. The chat GIF picker also gets viewport bounds.
Mobile received/history folder actions are hidden; sync chat rows retain file
information as noninteractive text. Desktop actions remain available.

No simulator installation or launch was performed in this verification pass;
real iOS keyboard and safe-area behavior still need an operator smoke test.

---

# Simulator build verification — 2026-09-09

Source: `209d9ab` (includes UIKit default name and safe-area fixes).

Commands run from this worktree:

```sh
npm run build
npx tauri ios build --debug --target aarch64-sim --no-sign --ci
```

Result: `src-tauri/gen/apple/build/arm64-sim/DropBeam.app`.
Bundle directory mtime: **2026-09-09 01:47:36.968 +08:00**.
Executable mtime: **2026-09-09 01:47:34.983 +08:00**.
Executable SHA-256: `7fe32b1d96d9fa30717318b11b943354a1217261b784260cd9121826bf9c83f6`.

Verified the Brotli asset embedded in the executable decompresses byte-for-byte
to `dist/assets/index-DfOPqIYw.js` and contains `tabbar-item` (MobileTabBar).
JS SHA-256: `bccebbb2fc51f5094f8ee9a20a504ddfca887a84509bf9bf25907437176903e6`.
Also verified embedded HTML contains `viewport-fit=cover` and the built plist
has `UIViewControllerBasedStatusBarAppearance=true` and
`UIStatusBarStyle=UIStatusBarStyleDefault`.

The iOS device-target Rust library check passed. SwiftPM initially failed inside
the execution sandbox (`sandbox_apply: Operation not permitted`); it passed when
run outside that sandbox. The first simulator packaging attempt collided with
the existing output (`Directory not empty`). Moved the old `arm64-sim` directory
to `build/arm64-sim-before-safe-area` and reran successfully.

Nonfatal warnings: existing Rust warnings during cargo check, frontend chunk size,
bundle identifier ending in `.app`, multiple matching Xcode destinations, and a
blake3 object built for simulator SDK 26.5 while the deployment target is 14.0.
Compatibility with older iOS versions remains unverified.

No simulator installation or launch was performed. The operator still needs to
check portrait/landscape safe areas, status-bar contrast in light/dark appearance,
and the first-run display name. Existing saved names are preserved.
