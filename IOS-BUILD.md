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
