# Plan — Remote drives, agent control, open feedback (2026-10-22)

Owner request (2026-10-22): make a remote drive (first case: the Windows OptiPlex with a
4TB `D:` drive, reached from the Mac over LAN or Tailscale) feel like a local drive —
fastest path, seamless, files open in place and stream — plus build a CLI/MCP so AI
agents get the same control a human has, and address every open feedback Issue.

Guiding rule for the agent work (from lman80/nucleus `docs/THESIS.md`, `docs/WHY.md`):
**AI-first means giving an agent the same verbs a human has — not building AI into the
app.** Complex apps run as services with their own MCP; every action a person can take
in the UI is an op an agent can call, with the same rules, written down (self-describing),
and recorded (an agent that acts without a trace is a bug).

## Phase 0 — Issue housekeeping
- Label every Issue worked on `building`; close each with a comment when fixed.
- Close #80 (empty report), #87/#88 (SIGABRT at launch on 0.52.2 with no stack, two
  builds behind 0.54.0) as not planned, asking for a new report if seen on ≥0.54.

## Phase 1 — Everyday fixes
- **#83** A device's chosen name (set during first-run) must be the name friends see.
  Find why a new Windows device is added under a random/fallback name (code payload
  `{eid,name}`, hello, `needsName` flow) and fix at the source; existing friends get the
  real name on next hello unless the user renamed them locally.
- **#81** A friend with several devices on one DropBeam account shows once (chat list,
  Friends, send targets). Sends go to whichever device is online (prefer the most
  recently active / LAN); a device picker lets the user pick a specific one. Chat
  history of the merged person is one thread. Desktop + iOS.
- **#84** Long chat messages: the copy/actions control sits at the bubble's bottom edge
  (sticky to the visible part), visible whenever the bubble is hovered; reachable by
  keyboard. Desktop + iOS long-press unchanged.
- **#85** QR scanning from a laptop camera: auto-zoom — scan a multi-scale set of center
  crops each frame (digital zoom ×1/1.5/2/3) and offer a zoom slider/pinch; same on iOS
  (`videoZoomFactor` ramp when a code-sized region is detected but not decoded).

## Phase 2 — Windows can host a Location (the OptiPlex case)
- Port `locations.rs` `Root` to Windows: handle-relative traversal (`NtCreateFile` with
  `RootDirectory`, or per-component open with `FILE_FLAG_OPEN_REPARSE_POINT` and refusal
  of reparse points/junctions/symlinks), volume-serial + file-index identity instead of
  dev/ino, no-replace publish via `MoveFileExW` without `REPLACE_EXISTING`, recoverable
  trash, mount marker, root protection (`C:\`, `%USERPROFILE%`, app config, `C:\Windows`,
  `Program Files`), Windows-reserved names and characters.
- `mounts.rs` on Windows: list fixed/removable/network drives with label + free space so
  the wizard suggests "Storage (D:)".
- Remove the Windows "isn't available yet" UI branches; same wizard as macOS/Linux.
- Run the existing Locations test suite on Windows (cfg(unix) tests ported where the
  behavior is shared) in CI.

## Phase 3 — Open in place and stream (no full copy first)
- Protocol `locations_v: 2` (v1 peers keep working, feature-detected by capability):
  - `locations.stat {id, rel_path}` → `{isDir,size,modified,etag}`.
  - `locations.read {id, rel_path, offset, len, etag?}` → header frame then raw bytes on
    the same stream; host reads from the pinned `Root` (same ACL/rate/marker checks as
    download); `etag` mismatch → `changed` error.
  - `locations.write` path for drive saves = stage locally, then the existing verified
    upload/publish (no partial writes on the host).
- Client **stream engine**: block cache (1 MiB blocks, on-disk LRU in the cache dir, size
  setting, default 10 GB), read-ahead that grows with sequential access, several
  concurrent range requests per file over the existing connection (QUIC streams),
  automatic best path (LAN → direct/Tailscale → relay, unchanged selector).
- Local **range server** on `127.0.0.1` (random port, unguessable per-run token) serving
  `GET/HEAD` with `Range` for any Location file → used by: in-app preview (images, PDF,
  audio, video with seeking), "Open" (opens in the default app via the drive mount, or
  the URL for players that accept URLs), and iOS (AVPlayer / QuickLook from the same
  server inside the app).
- UI: in the Locations browser, double-click / tap opens in place; video/audio stream
  immediately; "Download" stays for keeping a copy. Thumbnails for images/videos
  (host-generated small JPEG via `locations.thumb`, cached).

## Phase 4 — It shows up as a drive
- One virtual tree: `/<friend or own device>/<location>/…`, served by the stream engine.
- **macOS:** in-process NFSv3 server (`nfsserve` crate) on localhost, mounted without
  admin at `~/DropBeam Drives` (or `/Volumes`) with `mount_nfs` — true random-access reads,
  so video seeks and big files work; shows in Finder sidebar. Writes = stage + verified
  upload on close/commit.
- **Linux:** WebDAV server (`dav-server` crate) on localhost, mounted with `gio mount
  dav://…` (no root; appears in Files), plus the URL for any WebDAV client.
- **Windows:** the same WebDAV server mapped as a drive letter with `net use` (WebClient);
  if Windows' 50 MB WebDAV file-size limit or a stopped WebClient service is detected, a
  one-click "Allow large files" fix (elevated, explained).
- Settings → Drives: "Show friends' Locations as a drive" (on by default when you have
  any), auto-mount at launch, cache size, "Open drive" button in the Locations browser.
- iOS: no system-wide mount in this pass (needs a File Provider extension); in-app open
  in place + streaming via Phase 3. Recorded as a follow-up.

## Phase 5 — Agents get the same control as a person (#86, #82, #70)
- **Op registry** (`agent_ops.rs`): every user action as a named op
  (`friends.list`, `friends.add`, `chat.send`, `chat.read`, `files.send`,
  `transfers.list/cancel/pause`, `quicksend.create/receive`, `locations.list/ls/stat/
  download/upload/mkdir/rename/trash/read`, `drives.mount/unmount/status`,
  `folders.*`, `devices.list/link`, `settings.get/update`, `server.*`, …) with a JSON
  schema, description, risk (read / write / destructive), and the same validation the
  UI uses (it calls the same command functions). `ops.describe` returns the catalog.
- **Local control API**: the running app listens on a per-user socket (Unix socket in the
  config dir, 0600; Windows named pipe with an owner-only ACL) plus a token file.
  Every call is appended to `agent-actions.jsonl` and visible in Settings → Agents
  (who/what/when/result), and the user can turn agent access off.
- **CLI** `dropbeam` (same binary, `dropbeam <op> [--json args]` with friendly
  subcommands: `dropbeam send <friend|device> <files…>`, `dropbeam chat <who> "text"`,
  `dropbeam read <who>`, `dropbeam ls <friend>:<location>/path`, `dropbeam get/put`,
  `dropbeam drive mount`, `dropbeam wait-for message`…). Starts the app in the background
  if it isn't running. Installed on PATH by the app (Settings → Agents → "Install command
  line tool"; Linux/Windows installers add it).
- **MCP** `dropbeam mcp` — stdio MCP server exposing the op catalog as tools (generated
  from the registry, so CLI, MCP and UI never drift). Setup snippet for Claude Code /
  Codex shown in Settings → Agents.
- **Agent-to-agent messaging:** chat messages carry an optional `agent` marker; `dropbeam
  wait-for message --from X` lets agents converse through DropBeam.
- **New-computer bootstrap (#86): run commands on your own devices.** Only devices linked
  to the same DropBeam account; off by default on each device; the target device's owner
  turns on "Let my other devices run commands here" (with a time limit option); every
  command shows on the target as a notification and in its Agents log; `dropbeam exec
  <device> -- <command>` streams stdout/stderr/exit code. This turns "install DropBeam,
  scan the QR" into "my agent can now set up SSH on this machine".

## Phase 6 — Safe speed finder (#20)
- Settings → Speed limit → "Find my safe speed": ramps a test transfer to an online own
  device/friend while probing the LAN gateway (ping/ARP-level reachability + RTT). When
  the gateway's latency spikes or it stops answering, the test backs off, and the result
  recommends ~80% of the last healthy rate. Optional "Protect my router" mode keeps
  watching the gateway during real transfers and throttles automatically.

## Phase 7 — Transfer Server (#45)
- Verify what's left against `docs/TRANSFER-SERVER-PLAN.md`: setup flow, explanations
  ("What's this?" guides), offline detection speed, iOS push (APNs key status). Finish
  any gap; close #45 with the state, or leave open listing only owner-only steps.

## Phase 8 — Verification and docs
- `cargo test --lib`, `npm run build`, `npm run lint`, TS tests, clippy — all clean.
- Live test: Mac ↔ OptiPlex (Windows host of `D:\`), over LAN and over Tailscale only:
  browse, open a large video and seek, copy a 1 GB file both ways, mount the drive in
  Finder, CLI + MCP calls, `dropbeam exec` on an own device.
- Update `docs/FEATURES.md`, `docs/PARITY.md`, `docs/locations-phase1.md` (→ v2 notes),
  README; record deviations at the bottom of this file.

## Deviations / results
(filled in as work lands)
