# Locations — v2: open in place and the drive

Extends [locations-phase1.md](locations-phase1.md). Everything there still holds:
identity from `conn.remote_id()`, the location ACL, the verified mount marker and
pinned `Root`, per-friend token buckets, no-replace publication. Plan:
[PLAN-2026-10-22-DRIVE-AGENTS.md](PLAN-2026-10-22-DRIVE-AGENTS.md) Phases 3–4.

## Protocol

### Capability (feature detection)

v1 clients require `locations_v == 1` exactly in the ping/pong and hello, so
that field is unchanged. v2 is advertised **beside** it:

```
pong / friend-hello (+ reply):  {"locations_v": 1, "locations_max": 2, …}
```

A client sends a v2 operation only after `locations_capability(conn) >= 2`
(`iroh_net.rs`); an older host answers without `locations_max` and the UI says
"This device needs a DropBeam update to open files in place." Hosts accept
`locations_v` 1 or 2 on every request (`locations::request_version`); the v2
operations below refuse `locations_v: 1`. v1 requests from old clients are
served exactly as before (loopback test `loopback_open_in_place_stat_read_thumb_and_v1_interop`).

### Operations (all need the download right — any friend the location is shared with)

| Kind | Request | Reply |
| --- | --- | --- |
| `locations.stat` (no `paths`) | `{id, rel_path}` | `{isDir, size, modified(ms), etag}` |
| `locations.read` | `{id, rel_path, offset, len, etag?}` | header frame `{ok, size, etag, offset, len}` then exactly `len` raw bytes on the same bi stream |
| `locations.thumb` | `{id, rel_path, max(32–512)}` | `{etag, mime:"image/jpeg", data: base64}` |

The batched v1 `locations.stat {paths:[…]}` keeps its old shape.

- `etag` = size + mtime (ns) + inode (Windows: size + mtime). A `locations.read`
  whose `etag` differs is refused with `{ok:false, code:"changed"}`; the client
  drops that file's cached blocks, re-stats and retries once.
- `len` is clamped to 16 MiB (`MAX_READ`) and to the end of the file. The host
  reads with positioned reads on a blocking thread, up to 4 MiB ahead of the
  network, in 1 MiB chunks; a failed or short read resets the stream so a
  truncated range can't be mistaken for a whole one.
- Every request re-runs `authorize` + `validate_root` + `Root::for_location`
  (marker check) and opens the file through `Root::open_entry` → `open_rel`
  (descriptor/handle-relative, no symlinks/junctions/reparse points, same
  device). Revocation takes effect on the next request.
- Rate limits: reads have their own per-friend bucket (200/s, burst 200 — a seek
  is a handful of requests); `stat` shares the `ls` bucket (10/s); thumbnails
  have their own 10/s bucket. Clients back off and retry on "rate limit".
- Thumbnails: JPEG/PNG/GIF/BMP/WebP up to 64 MiB, decoded with the `image` crate
  already in the tree; no video poster frames (no decoder in the tree). Cached on
  the host in `<config>/location-thumbs/` by (location, path, etag, size), at
  most 4,000 files.

## Client stream engine (`src-tauri/src/stream.rs`)

- 1 MiB blocks as files in `<app cache>/stream-blocks/<xx>/<key>-<n>`, key =
  sha256(device, location, path, etag). LRU over all blocks, limit from
  Settings → Drives (default 10 GB, 1–2000 GB); rebuilt from disk at launch.
- Concurrent readers of one block share one request (in-flight map + watch
  channel). Missing blocks become range requests of up to 4 blocks, up to 8 at
  once per device, each on its own QUIC stream of the friend's connection.
  Fetches are spawned, so a reader that gives up (a player seeking away) never
  strands others waiting on the same blocks.
- Read-ahead: a sequential reader's window doubles from 2 to 64 blocks; a seek
  resets it; re-reading what was just read doesn't count as progress; never
  more than a quarter of the cache.
- Path selection is iroh's unchanged selector; `stream_status` reports the
  current path (`local`/`direct`/`relay`, from `conn_detail`), RTT and a 2-second
  throughput average.
- The connection is the cached friend connection, verified once
  (`stream_connection`) and redialed once on a dropped stream.

### Local range server

hyper (already in the tree via reqwest) on `127.0.0.1:<random port>`; every URL
starts with a 48-hex-digit per-run token; the Host header must be
127.0.0.1/localhost (DNS rebinding).

- `GET/HEAD /<token>/f/<friendId>/<locationId>/<path…>` — one `bytes=` range →
  206 + `Content-Range`; several ranges → 200 with the whole body (RFC 9110
  allows it); past the end → 416. `Accept-Ranges`, `ETag`, `Content-Type` by
  extension; HTML/SVG/scripts are served as `text/plain` and every text or
  unknown type gets `Content-Security-Policy: sandbox` and `nosniff`. CORS only
  for the app's own webview origins.
- `/<token>/dav/…` — the drive over WebDAV (below).

The webview CSP allows `http://127.0.0.1:*` for images, media, fetch and frames.
`frame-src` is needed only for the PDF preview (an `<iframe>` on the range
server; nothing else is framed). The port is random per run (`bind :0`), so the
CSP can't name it; a fixed range would collide with other programs. PDF
responses deliberately do **not** carry `Content-Security-Policy: sandbox`: the
webviews' built-in PDF viewers are plugins, and a sandboxed document can't run
plugins (no sandbox token lifts that), so the preview would go blank. A PDF is
shown on the range server's origin (`http://127.0.0.1:P`), not the app's, so it
can't reach the app's IPC; it is same-origin with other range-server URLs, which
need the per-run token. iOS: `NSAllowsLocalNetworking` so AVPlayer may use plain HTTP to 127.0.0.1.

## The drive (`src-tauri/src/drive.rs`)

One tree: `/<device name>/<location name>/…` over every friend device that shares
a Location with this one (names made safe and unique; a device that's offline
keeps what it shared last time). File ids for NFS handles are persisted in
`<config>/drive-ids.json` and follow renames.

- **macOS:** in-process NFSv3 server (`nfsserve` 0.11, BSD-3-Clause, maintained
  by Hugging Face for hf-mount) on 127.0.0.1, mounted as the signed-in user:
  `mount_nfs -o locallocks,vers=3,tcp,port=P,mountport=P,soft,intr,timeo=300,retrans=2,deadtimeout=60,rsize=1048576,wsize=1048576,actimeo=5 127.0.0.1:/ ~/DropBeam\ Drives`
  (hf-mount's options plus a soft mount that is dropped after `deadtimeout` if
  DropBeam stops answering). Finder shows the volume as "DropBeam Drives".
  Unmount: `umount`, then `diskutil unmount force`. A mount left by a crash is
  found in `mount` output at launch and removed.
- **Linux:** `gio mount dav://127.0.0.1:P/<token>/dav/` (gvfs, no root; appears in
  Files); unmount `gio mount -u`; stale `dav://127.0.0.1:` mounts removed at launch.
- **Windows:** `net use X: \\127.0.0.1@P\<token>\dav /persistent:no` (WebClient).
  `sc qc WebClient` / `FileSizeLimitInBytes` are checked; **Allow large files**
  runs one elevated PowerShell step (WebClient automatic + restarted, limit
  raised to the 4 GB maximum). Stale `\\127.0.0.1@` mappings removed at launch.
- The WebDAV view is served whenever the app runs (it is also the URL for any
  WebDAV client: `drive_status().davUrl`); locks are dav-server's FakeLs.

### Writes (commit semantics)

NFSv3 has no close, and nfsserve answers every WRITE as FILE_SYNC (so clients
send no COMMIT; nfsserve doesn't implement it). Therefore:

1. CREATE / first WRITE / truncate makes a private staging file in
   `<app cache>/drive-staging/<uuid>/<name>` (an existing remote file is fetched
   through the block cache first when written without truncation).
2. When the file has had no writes for **2 seconds** (or a WebDAV PUT ends), it
   is uploaded with the existing Locations upload (`start_location_upload_replacing`
   → hashed, verified, no-replace publication). Saving over an existing file
   uses `replace_existing`, so the host's previous version goes to the
   location's recoverable trash. A write during an upload edits a fresh copy
   that uploads next.
3. Until the host lists the file at the uploaded size, the drive shows the
   local copy; then the staging copy is deleted. A failed start is retried
   (Settings → Drives shows the reason).

`mkdir` / `rename` / `remove` map to `locations.mkdir` / `rename` / `trash`
(manage right, else EACCES); without the upload right the location is
read-only (EROFS). Renames across locations/devices answer EXDEV. Finder/Office
housekeeping files (`.DS_Store`, `._*`, `~$*`, `Thumbs.db`, `desktop.ini`,
`.~lock.*`) stay on this device for the session and are never uploaded.

## Commands (for the UI, the iOS bridge and the agent layer)

`location_stat`, `location_read_url` → `{url, kind, mime, size, etag}`,
`location_thumb` → data URL | null, `location_cache_file` → path,
`location_open`, `stream_status`, `stream_clear_cache`, `drive_status`,
`drive_mount`, `drive_unmount`, `drive_refresh`, `drive_settings_set`,
`drive_open {friendId?, locationId?, relPath?}`, `drive_fix_windows`.

## Security

- **Through the mount (macOS):** every file and folder is reported as owned by
  the signed-in user with owner-only modes (folders 0700/0500, files
  0600/0400), and `~/DropBeam Drives` is created 0700 (an older 0755 one is
  tightened at mount). The macOS NFS client checks mode bits itself (only
  `access(2)` asks the server), so other users on the Mac can't list or open
  anything through the mount.
- **The raw NFS port is not protected.** NFSv3 as nfsserve 0.11 implements it
  has no authentication: AUTH_UNIX credentials are whatever the caller claims,
  ACCESS answers "yes" to everyone, and file handles are (server start time,
  file id). Any process on this computer — including another user's — that
  speaks NFS to the `127.0.0.1` port can read, write, rename and delete in
  every shared Location this device can reach, as this user. An unguessable
  export path was considered and not done: nfsserve's MOUNT `EXPORT` call lists
  the export, and the mount source (`127.0.0.1:/<path>`) plus port are visible
  to every local user in `mount` output, so it would hide nothing. Nothing off
  this computer can reach the port (it listens on 127.0.0.1 only). The
  HTTP/WebDAV server (Linux/Windows drive, previews) is not affected: every URL
  needs the per-run token. Same exposure as hf-mount; single-user Macs are the
  target. Turn the drive off on a shared Mac.

## Limits and known gaps

- Commit is idle-based: a program that pauses more than 2 s in the middle of
  writing one file causes an extra (verified) upload of the partial file,
  replaced by the complete one afterwards.
- Linux/Windows mounting is compiled and unit-tested in CI (WebDAV handler tests
  run on all three) but was not live-mounted in this pass. iOS has no
  system-wide drive (needs a File Provider extension).
- No video poster frames for thumbnails.

## Verification (macOS dev host, 2026-10-22)

- `cargo test --lib`: all pass, including stream engine (range parsing, LRU,
  coalescing, read-ahead, etag invalidation, restart), drive tree (path mapping,
  ids, staging/commit, rights, WebDAV PROPFIND/GET/PUT) and the QUIC loopback.
- Manual end to end (`drive_e2e_on_this_mac`, two engines with separate config
  dirs over QUIC loopback, real `mount_nfs`): a 1 GiB file read through the
  mount matched sha256 (cold, ~80 MB/s debug build); 5 random 3 MiB `dd` ranges
  matched; `ffprobe` read the 60 s 1080p video; `ffmpeg -ss` seeks to 5/31/52 s
  took 0.14–0.34 s; a 50 MB `cp` into the mount returned in 0.23 s and was
  published on the host, sha256-verified, 4.3 s later; overwrite moved the old
  version to `.dropbeam-trash`; mkdir/rename/delete reached the host; Finder
  listed the volume; unmount was clean.
