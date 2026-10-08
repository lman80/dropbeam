# DropBeam

DropBeam sends files, folders, photos and messages **directly between your devices and your friends' devices**, on your local network or across the internet, over end-to-end encrypted peer-to-peer connections. You don't need a cloud account, and there's no upload-then-download.

It runs on **macOS, Windows, Linux** and **iPhone/iPad** (TestFlight).

- **Send to friends by name.** Add a friend once by code or QR. After that you pick their name and send. Big files resume after interruptions, and the result is checked end to end.
- **Quick Send.** Get a link or QR code for someone who isn't a friend yet.
- **Chat.** Messages, reactions, replies, edits, read receipts, GIFs, link previews, and files inline.
- **Shared folders.** A folder that stays in sync with friends (two-way or view-only), with per-person roles, pause/resume and recoverable deletes.
- **Locations.** Share a NAS or folder with friends so they can browse it and upload to it.
- **Your devices, one account.** Link your Mac, PC, Linux box and iPhone. Friends and conversations stay in step between them directly, with no server.
- **Transfer Server (optional).** An always-on DropBeam device (a home PC, NAS or Linux box) holds encrypted items for devices that are offline and passes them on later. It can also wake an iPhone with a push notification.

The full guide is [docs/FEATURES.md](docs/FEATURES.md).

## Download

Get the latest version from the **[Releases page](https://github.com/lman80/dropbeam/releases/latest)**:

| Platform | Download |
|---|---|
| macOS 10.15+ (Apple Silicon & Intel) | `.dmg`. Open it and drag DropBeam to Applications. |
| Windows 10/11 (x64) | `-setup.exe` (or `.msi` for managed installs) |
| Windows on ARM | `arm64-setup.exe`, when a release includes it |
| Linux x86_64 / aarch64 | `.deb` (Debian, Ubuntu), `.rpm` (Fedora, openSUSE) or `.AppImage` (any distro) |

After that, **updates install themselves** from inside the app.

**Linux notes**
- **AppImage** needs FUSE 2. On Ubuntu 22.04 run `sudo apt install libfuse2`; on 24.04 and later run `sudo apt install libfuse2t64`. Then `chmod +x DropBeam_*.AppImage` and run it.
- **Tray icon:** GNOME needs the *AppIndicator and KStatusNotifierItem Support* extension (Ubuntu ships it). Without a tray, DropBeam keeps its window, and closing the window minimizes it instead of hiding it.
- **Right-click → Send with DropBeam** works in KDE Dolphin and Cinnamon Nemo with the `.deb`/`.rpm`. In GNOME Files use *Open With… → DropBeam*.
- **Blank window on NVIDIA:** DropBeam disables WebKitGTK's DMA-BUF renderer automatically on NVIDIA's driver. Elsewhere, start it with `DROPBEAM_SOFTWARE_RENDER=1`.
- **Wayland:** the compositor decides where windows go, so the small transfer pop-ups may not sit exactly in the corner.

**Unsigned builds.** Releases built before code signing is set up (see [docs/SIGNING-SETUP.md](docs/SIGNING-SETUP.md)) are blocked once by the OS:
- **macOS:** 1. Open DropBeam. 2. When it says *"Apple could not verify…"*, click **Done**. 3. Open **System Settings → Privacy & Security**, scroll down, and click **Open Anyway** next to DropBeam. 4. Click **Open Anyway** again and enter your Mac password. You only do this once.
- **Windows:** 1. Run the installer. 2. If a blue *"Windows protected your PC"* box appears, click **More info**. 3. Click **Run anyway**. You only do this once.
- **First launch on a Mac:** when macOS asks to let DropBeam *find devices on your local network*, click **Allow** — otherwise sending to computers in the same house is slow. If you clicked Don't Allow, DropBeam shows a banner with an **Open Settings** button.

**Headless Transfer Server (Linux):** the `.deb` installs a systemd unit. Enable it with `sudo systemctl enable --now dropbeam-server@$USER` to run `DropBeam --server` without a window.

## Privacy

Your files and messages go device to device and are never stored on our servers. Some things do leave your device. Diagnostics are **on by default** (a redacted error/performance summary; turn them off in Settings → Privacy). Feedback you send becomes a **public** GitHub issue. Relay and discovery servers see your IP address and device id. Read **[PRIVACY.md](PRIVACY.md)** for the full list.

## How it works

- **Transport:** [iroh](https://iroh.computer) (QUIC) with NAT hole-punching. Devices on the same network find each other with mDNS and connect directly. Across the internet they connect directly when possible and fall back to an encrypted relay otherwise. Relays are number0's public ones by default, or your own ([RELAY-SETUP.md](RELAY-SETUP.md)).
- **Identity:** each install has an Ed25519 key, which doubles as its device id. Friends are pinned keys exchanged by code or QR. Linked devices are recorded as one account and sync friends and chats with each other directly.
- **Integrity:** received files are hashed (SHA-256) and verified, partial transfers resume, and shared-folder deletes have a freshness guard plus a local recovery archive.
- **Transfer Server:** items are sealed (X25519) to the recipient device before upload, so the server holds ciphertext only. Held items expire after 14 days (files) and 30 days (chat) by default.

### Stack

- **Tauri v2.** The Rust backend is in `src-tauri/`; the web UI is React 19 + TypeScript + Vite + Tailwind v4 + Framer Motion in `src/`.
- **iOS:** a native SwiftUI shell over the same Rust engine (Tauri plugin `src-tauri/plugins/native-ui`). See [IOS.md](IOS.md).
- **Vendored crates** with small DropBeam patches, each with a `DROPBEAM-PATCH.md` or a "DropBeam patch" comment: `iroh`, `noq`, `noq-proto`, `netwatch`, `tauri-plugin-notification` (in `src-tauri/vendor/`).

Main Rust modules: `iroh_net.rs` (transport, transfers, chat wire), `sync.rs` (shared folders), `locations.rs` / `location_sync.rs`, `mailbox/` (Transfer Server), `link.rs` / `account.rs` (device linking), `friends.rs`, `chat.rs`, `telemetry.rs` (diagnostics), `desktop_shell.rs` (tray, popover, deep links), and `commands.rs` (UI commands).

## Build from source

Prerequisites:
- **Rust.** The version is pinned in `rust-toolchain.toml`; rustup installs it automatically.
- **Node 22.**
- **Linux:** `sudo apt install libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev librsvg2-dev libayatana-appindicator3-dev patchelf`

```bash
npm ci
npx tauri dev                       # hot-reload dev app
npx tauri build                     # release installers for this OS
cd src-tauri && cargo test --lib -- --test-threads=1   # unit + loopback engine tests
```

On macOS, build from a path **without spaces or parentheses**, because the DMG and codesign tools break on them.

**Releases:** bump the version in `src-tauri/tauri.conf.json`, `src-tauri/Cargo.toml` and `package.json` (CI refuses a tag that doesn't match all three). Then push a `vX.Y.Z` tag. `.github/workflows/release.yml` builds every platform into a draft release and publishes it once the builds succeed. The in-app updater picks it up from `latest.json`. Code signing is set up in [docs/SIGNING-SETUP.md](docs/SIGNING-SETUP.md).

Third-party licences are listed in [THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md). To regenerate the file, run `node scripts/third-party-notices.mjs`.

## Licence

DropBeam's own licence hasn't been chosen yet. See [docs/LICENSE-DECISION.md](docs/LICENSE-DECISION.md). Until a licence is added, all rights are reserved. Third-party components are under their own licences ([THIRD-PARTY-NOTICES.md](THIRD-PARTY-NOTICES.md)).

## Feedback

Use **Send Feedback** inside the app, or open an issue at <https://github.com/lman80/dropbeam/issues>. Both are public.
