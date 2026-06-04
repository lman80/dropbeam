# DropBeam

A polished, cross‑platform desktop app for sending files directly between
computers — on your local network or anywhere over the internet — with
end‑to‑end encryption. Built on [croc](https://github.com/schollz/croc).

Two modes:

- **Quick Send** — drag a file in, get a short code, the other side enters it. Works
  on the LAN (full speed) or across the internet (direct P2P, relay fallback).
- **Shared Drop Folders** — pair a folder with a friend; anything dropped in is
  auto‑beamed to their paired folder, with an optional "self‑emptying outbox"
  (delete the local copy once delivery is confirmed).

No accounts, no telemetry, no server to host. Files never touch our servers — the
public croc relay only brokers the encrypted connection (PAKE), it can't read data.

---

## Stack

- **Tauri v2** (Rust backend + web frontend) — small, fast, native.
- **React 19 + TypeScript + Vite + Tailwind v4 + Framer Motion** frontend.
- **croc** bundled as a Tauri *external binary* (sidecar), one per platform.
- Rust: `tokio` (process orchestration), `notify` (folder watching), `regex`
  (progress parsing), `trash` (recoverable delete), `sha2` (code derivation).

## Build & run

Prereqs: Rust (stable) and Node 18+.

```bash
npm install
npx tauri dev            # hot‑reload dev app
npx tauri build          # release .app + .dmg (macOS), .msi/.exe (Windows)
```

> **Build from a path without spaces or parentheses.** macOS `.dmg`/codesign
> tooling breaks on paths like `…/DropBeam (Blip but Better)/`. This project is
> developed in `~/DropBeam` for that reason.

### The croc sidecars

`src-tauri/binaries/` holds the croc binary for each target, named with the Rust
target triple so Tauri bundles the right one:

```
croc-aarch64-apple-darwin        # macOS Apple Silicon
croc-x86_64-apple-darwin         # macOS Intel
croc-x86_64-pc-windows-msvc.exe  # Windows x64
```

`scripts/fetch-croc.sh` downloads the current release for each platform.

---

## croc integration — the specifics

These are the three things worth calling out (per the original spec).

### 1. Prompt‑free recurring transfers between paired peers

croc is normally one code phrase per transfer. To make a *paired folder* sync
without codes, each pair stores a shared high‑entropy `secret` (created at pairing
time, carried in the invite). Per‑direction transfer codes are **derived** from it:

```
code = hex( sha256(secret ":" channel) )[..24]      # ≥ 6 chars, croc's minimum
```

The inviter is role **A**, the accepter is role **B**. A sends on channel `a2b`
and listens on `b2a`; B is the mirror — so the two directions never collide, and
both sides compute the *same* code for a given direction without any signaling
(unit‑tested in `src/pairing.rs`).

Rendezvous uses croc's natural asymmetry: **the sender parks** (`croc send` waits
for a receiver), **the receiver polls** (`croc <code>` gives up after ~2s if no
sender, so the listener loop re‑polls with an adaptive backoff). When a file is
queued, the sender keeps trying; the listener catches it on its next poll. The
shared code is passed via the `CROC_SECRET` env var (never argv, so it can't leak
in the process list).

### 2. Confirming delivery before auto‑delete

`croc send` blocks until the receiver sends its `TypeFinished` handshake, i.e. it
**exits 0 only after the receiver has the whole file**. So the engine gates
auto‑delete on the sender process exit code: a local copy is trashed *only* after
`exit == 0`. If a transfer is interrupted or the peer is offline, the file stays
queued and is retried — it is **never** deleted before confirmed receipt.

### 3. Local (LAN) vs internet detection

croc prints the connected peer endpoint to stderr (`Sending (->192.168.1.74:…)` /
`Receiving (<-…)`). The engine parses that IP and classifies it: an RFC‑1918 /
link‑local address → **Local network** badge; a public address → **Internet**
badge. (All of croc's human output — code, progress bar, peer line — is on
**stderr**, repainted with `\r`; the engine reads stderr raw and splits on `\r`/`\n`
to surface true live progress. See `src/croc.rs`, with parser unit tests.)

---

## Architecture

**Rust core** (`src-tauri/src/`):

| module | responsibility |
|---|---|
| `croc.rs` | Quick Send/receive engine — spawn croc, parse progress, emit events, cancel |
| `sync.rs` | Shared Drop Folder runtime — watcher, send queue, listener loop, auto‑delete, loop guard |
| `pairing.rs` | Pair persistence, invite encode/decode, derived codes |
| `settings.rs` / `history.rs` | JSON persistence (atomic writes) |
| `commands.rs` | Tauri commands invoked from the UI |
| `lib.rs` | App wiring, tray/menu‑bar, hide‑to‑tray, autostart |

**Frontend** (`src/`): `store.ts` (zustand) subscribes to backend events
(`transfer://update`, `folder://status`, `history://changed`); views in `views/`,
components in `components/`. `lib/mock.ts` lets the UI run in a plain browser for
development (auto‑activates when the Tauri APIs are absent).

### Shared Drop Folder runtime (per pair)

- A `notify` watcher (debounced + write‑completion via size‑stability) feeds an
  ordered send queue. The queue is effectively the folder itself: undelivered files
  stay on disk and are re‑queued on launch.
- A **persistent manifest** (`synced-<id>.json`) records the signature
  (`relpath|size|mtime`) of every file sent or received, so restarts don't re‑send
  delivered files and a two‑way folder never beams a *received* file back.
- The listener receives into a hidden `.dropbeam-incoming/` staging dir, then moves
  arrivals into the folder collision‑safely (`name (1).ext`).
- Auto‑delete routes to the OS Trash by default (recoverable), with a verify‑then‑
  fallback so it still clears files from locations Trash can't handle.

---

## Security & privacy

- All transfers are end‑to‑end encrypted by croc (PAKE‑derived keys); the relay
  can't read file contents.
- Shared Drop Folders only auto‑accept on the derived codes of explicitly paired
  peers. Incoming files are written into a staging dir and validated before moving
  into the folder.
- Pairing secrets are stored in the app's private config dir. (A future hardening
  is to move them into the OS keychain.)
- No analytics, no accounts, no data leaves your machines except the transfer.

## Windows

The bundle config and the Windows croc sidecar are in place; `npx tauri build` on
a Windows machine produces `.msi`/`.exe`. (Cross‑compiling a signed Windows
installer from macOS is out of scope — build on Windows.)

## Known limitations / future work

- Paired folders keep a lightweight background connection (the listener polls);
  this is intentional given croc's model. Polling eases off when idle.
- Multi‑peer folders (one folder → several friends) are not in v1; the pairing
  model is designed to allow it later.
- Code signing/notarization needs an Apple Developer ID (not included). The app
  runs locally unsigned; for distribution, sign + notarize from a clean path.
