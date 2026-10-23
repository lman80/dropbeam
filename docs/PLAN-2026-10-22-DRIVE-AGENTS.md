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

- **Phases 3–4 (branch `stream-drive`)** — details in [locations-v2.md](locations-v2.md).
  - v2 is advertised as `locations_max: 2` beside the unchanged `locations_v: 1`
    (v1 clients compare `locations_v == 1` exactly; bumping it would have broken them).
  - No separate `locations.write`: drive saves stage locally and go through the
    existing verified upload (`replace_existing` for overwrites). Commit = 2 s
    write-idle (nfsserve has no COMMIT) or end of a WebDAV PUT.
  - Thumbnails: images only (no video decoder in the tree).
  - The range server and the WebDAV drive share one 127.0.0.1 hyper server
    (per-run token). Linux/Windows mounting compiled + unit-tested in CI, not
    live-mounted; macOS NFS verified end to end on the dev Mac.
  - Settings → Drives is its own tab; drive settings live in `drive.json`
    (not `settings.json`) to keep the change separate.
- **Phase 5 (agents, branch `agent-ctl`):** built as planned (docs/AGENTS.md). Deviations:
  no separate token file — the socket lives in an owner-only folder (0700/0600 plus a peer-uid
  check) and the Windows pipe's random name is in a user-only file with a user-SID-only ACL;
  `dropbeam wait-for message` is `dropbeam wait`; sending to a person with several devices
  follows the window (every device, online ones first) instead of "the online one" — `--device`
  picks one.
- **No PowerShell on Windows (branch `pwsh-free`):** on a real Windows 10 PC, AVG's behaviour
  shield flagged DropBeam's elevated `powershell -EncodedCommand` (background task) as
  `IDP.HELU.PSE95` and deleted DropBeam.exe. Replaced with native APIs: Task Scheduler COM
  (task XML, S4U) for the startup task, registry + Service Control Manager for the WebDAV fix,
  `HKCU\Environment` + `WM_SETTINGCHANGE` for the CLI PATH, WNet for drive letters, and Win32
  calls instead of `taskkill`/`reg`/`attrib`. Admin steps re-launch DropBeam.exe itself with
  "runas" and one strictly-checked internal argument (`--background-task …`, `--fix-webdav`;
  elevate.rs); a unit test fails if a script host or system tool reappears in that code.
- **Phase 1 (fixes):** #83 root cause was the first-run name screen marking you named even when
  the save was refused while settings loaded (plus a stale invite code and a hello reply without
  the name); #81 grouped devices only after each one's own hello — the hello reply now carries the
  signed account proof and accepting a request keeps it. #84/#85 as planned (desktop + iOS).
- **Phase 2 (Windows hosts):** as planned; every reparse point is refused (also hides OneDrive
  placeholders), 8.3 short names can't reach `.dropbeam-*` (security fix). Added: **Keep running
  when no one is signed in** for Windows (`background_host.rs`, a boot Scheduled Task running
  `DropBeam.exe --server --config …` as the user via S4U, cross-session handover by stop file).
- **Phase 6:** desktop only (iOS keeps its plain upload limit).
- **Phase 7:** the Transfer Server was already complete; added the animated "your iPhone gets a
  notification" guide. Owner-only: confirm push on a real iPhone.
- **Security review** (after merge): exec re-checks permission every 2 s and stops when the asker
  leaves; device linking/joining is window-only (an agent can't add a device to the account);
  macOS drive is owner-only (0700); stranger exec refusals are rate-limited and not logged
  verbatim; the CLI verifies it talks to the user's own DropBeam. Residual: any local process can
  reach the macOS NFS port (nfsserve has no auth).
- **Live tests (2026-10-22):** Mac ↔ Linux (ass-box, another network): friends added, Location
  shared, browsed, 1 GB streamed with a matching sha256 (as fast as plain SSH on that link), the
  drive auto-mounted in Finder, random reads matched, a file written on the Mac landed on Linux.
  Windows (OptiPlex): the build ran headless from SSH, the CLI worked, `D:\` was shared as a
  Location ("Storage (D:)" suggested) and the name became "Old Desktop" — then AVG deleted
  DropBeam (first for the PowerShell behaviour, then `IDP.Generic` for every DropBeam build,
  including the official 0.54.0). A Mac ↔ Windows streaming test could not be completed.
