# Locations — Phase 1

Locations expose explicitly selected folders through authenticated DropBeam friend
connections. They are independent of Shared Folders: no watching, mirroring, or
propagated deletion. Existing version fields and ALPN `dropbeam/1` are unchanged.

## Use

On the host, open **Settings → Locations → Add location**. Choose a mounted folder,
name it, and select each friend device that may access it. A new location has no
recipients. Browse/download is implicit for selected devices; upload defaults on,
manage defaults off. For the Ubuntu/GNOME example, choose the actual mounted path:
`/run/user/1000/gvfs/sftp:host=buddy-files/shares/<share>`.

On the Mac or another friend device, open **Locations**, select the host's folder,
then browse or upload. The narrow layout includes Locations in the navigation bar.
Select checkboxes for batch downloads or trash; click a folder name to open it.
Downloads and uploads appear in **Send & Receive**. Upload cards retain the
location and relative destination for Retry. Dropping files while a browser is
open uploads into that browser's current folder.

“Move to trash” is only offered with manage permission. Its confirmation names the
items and explains recovery. The host moves each item to
`<root>/.dropbeam-trash/<milliseconds>-<uuid>/<original-name>`. Restore it manually
on the host. There is no permanent-delete protocol or automatic trash purge.
“Stop sharing” removes configuration and moves its verified mount marker into recoverable trash. If the mount is unavailable, sharing still stops and marker cleanup is logged for the owner. User files remain untouched.

## Protocol

All JSON uses the existing length-prefixed `write_frame`/`read_frame` transport,
with one request and response per bidirectional stream. Identity comes exclusively
from `conn.remote_id()`, mapped to a local friend record. A request's claimed
friend id, display name, or endpoint id cannot grant permission.

`friend-hello` advertises `locations_v: 1` and `locations_changed: true`. Hello
receivers invalidate the Locations UI; config edits broadcast the existing
profile hello, including to revoked recipients so their lists are invalidated.
No folder paths or ACLs are included in announcements. Presence uses the existing
friend presence machinery. A compatible `ping`/`pong` exchange also advertises
`locations_v: 1`. Clients require a live capability response before sending any
Locations operation or upload header, so an older peer cannot accidentally
receive a location upload into Downloads after ignoring an unknown header field.

RPC requests include `kind` and `locations_v: 1`. Responses are
`{ok:true, data:... , locations_v:1}` or `{ok:false,error:...}`.

| Kind | Additional request fields | Result / required permission |
| --- | --- | --- |
| `locations.list` | none | Allowed `{id,name,rights:{upload,manage}}` records; authenticated friend |
| `locations.ls` | `id, rel_path, cursor?`, `page_size?` (1–500), `sort?` (name/size/modified), `query?` | `{entries:[{name,isDir,size,modified}],cursor,nextCursor,hasMore,total}`; browse |
| `locations.download` | `id, paths:[relative paths]` | `{transferId,skipped}` for the host's normal friend send (null id if all skipped); download |
| `locations.mkdir` | `id, rel_path` | Empty object; manage |
| `locations.rename` | `id, rel_path, to` (relative destination) | Empty object; manage |
| `locations.trash` | `id, rel_path` | `{trashPath}` relative to root; manage |

Uploads use the normal `files` header/body, adding `locations_v:1`,
`location:{location_id,rel_path}`, `location_transfer` (opaque transfer id), and a
`sha256` hex digest on each manifest item. Download snapshots use normal friend
send with `location_download:true`, `locations_v:1`, and the same digests. They do
not require a chat file-note. The normal classic/parallel engine, Direct/Relay
selection, cancel controls and auto-resume remain in use. Payload hashes are new
for this extension; the previous engine only used SHA-256 for resume identities.

Progress reserves the final 10% for verified NAS publication (upload) or hash
verification (download), including after resume. The sender receives completion
only after this step succeeds. Hashing and copying report real work to keep slow
NAS operations from appearing stalled. Upload staging is scoped to authenticated
sender, destination and content manifest, and is leased to one receive at a time.
Both automatic and manual retries can reuse it until the next cache sweep. An identical already-landed file
is verified and reused; a different existing file fails without replacement.

## Filesystem boundary

`locations.json` is persisted atomically in the config directory. An unreadable
or corrupt file causes an error rather than being overwritten with defaults.

Paths are validated before normalization. Absolute paths, NUL, any `..` component,
and reserved `.dropbeam-*` components are rejected. Colon and backslash are valid
Unix filename characters; they are rejected only on Windows. Unicode filenames are preserved; empty and `.` components normalize.
The root and existing target/parent are canonicalized. Unix traversal then uses
pinned directory descriptors, `openat`/`mkdirat`, `O_NOFOLLOW`, regular-file/directory
checks and device-id checks on every component. Symlinks are refused even when
they currently point inside the root. Nested mounts with a different device id
are refused; same-device bind mounts cannot be distinguished by that check.

At save, roots equal to or containing `/`, `$HOME`, the app config directory
(and `~/Library` on macOS) are refused. Roots inside the config directory or anywhere inside `~/Library` on macOS are also
refused. All paired folders are excluded in both directions, even if mirroring is
currently off, because their settings can change. These checks run on operations
too. A unique `.dropbeam-mount-<uuid>` marker is the persisted mount identity;
operations verify it before probing or accessing the share. Network remounts may
change `st_dev`: a matching marker is accepted and the device number is re-stamped
in memory (and persisted on save). Device/inode checks pin only an open session.
Existing unpinned locations require a save in Settings. A missing/changed marker
fails closed, including on an ordinary settings save. Markers are never
periodically garbage-collected from the NAS; explicit Stop sharing retires only
the verified marker via a trash move.

Writes from the existing transfer engine land in private config-directory staging,
never directly on a NAS path. Publication copies into an exclusive hidden temp
file on the NAS, checks SHA-256 and fsyncs. Native no-replace capability is cached
per destination device/inode for the process, with at most 128 pinned directory
descriptors. An upload retains its verified Root and capability throughout
publication; eviction cannot cause per-file probes. A remounted device receives a
new cache key. Probe failures, including EROFS/EACCES/EPERM and unknown errnos,
select reservation capability without preventing browsing or downloading.
Linux uses `renameat2(RENAME_NOREPLACE)`; macOS uses `renameatx_np(RENAME_EXCL)`.
Settings shows **Safe publish: native / reservation** per location (or an
unavailable reason if the root cannot be opened).

On filesystems such as gvfs-fuse, CIFS and some NFS/SMB mounts that reject the native
primitive with ENOSYS/EINVAL/ENOTSUP, file publication first tries atomic `linkat`
followed by unlinking the source name. An occupied name fails without replacement.
If hard links are also unsupported (or the source is a directory), a clear log
records use of the reservation fallback. That fallback first creates the
**final destination** with `O_CREAT|O_EXCL`. An existing file, directory or symlink
causes refusal. It then renames the staged file (or existing source for rename/trash)
over that reservation. A directory source uses an exclusive `mkdirat` reservation;
filesystems that cannot rename onto an empty directory fail with a clear error.
New folders use exclusive `mkdirat` directly. An empty destination that blocks a
nonempty upload is preserved and clearly logged with its device/inode; without
proof of ownership it cannot be reclaimed automatically. Failed publication removes only an
unchanged reservation, and failed trash moves remove the empty generated bucket.
Created share files use parent permission bits restricted to 0666, directories use
parent bits restricted to 0777, both subject to the process umask. Config cache
directories stay 0700; snapshot files are 0600.

**Reservation safety boundary:** device/inode and emptiness checks detect a changed
reservation before rename, but POSIX `renameat` has no conditional inode argument.
An independent NAS writer can replace or write the reserved destination between
the final check and rename. Therefore this requested fallback cannot provide an
absolute no-data-loss guarantee against concurrent external destination changes.
Native no-replace publication avoids replacement of an existing destination. Do
not treat a reservation capability label as proof of unconditional NAS safety.

Publication/manage locks are scoped to config directory + location ID and shared
with configuration revocation. Upload copying and hashing never hold them; they
cover namespace publication and settings changes. Upload authorization and mount
rechecks run before taking the lock, at most once per second during publication.
A per-location settings revision forces an immediate recheck after local ACL,
cap, root or Stop sharing changes. Waiting for the lock cannot bypass revocation. Listing takes no
publication lock. Download rights are checked before/after snapshot preparation
and before every send header, including retries. Successful mkdir/rename/trash
emit `locations://changed` with friend ID, location, item and operation. Settings
loads the last 50 in-memory activity records and subscribes to new events.

Listings collect at most 100,000 supported entries, sort/filter the **whole listing**,
and return at most 500 entries per page. Opaque cursors bind to the authenticated
friend, location, config directory and relative path. They reference a cached
snapshot for 120 seconds, with at most 128 cursor records and 200,000 total cached
entries globally (shared snapshots count once). Previous and Next use that
snapshot; expired/evicted cursors require Refresh. Listing, download preparation,
and management each use a per-friend token bucket with burst capacity 10 and
refill 10/second. Mkdir/rename/trash share one management bucket. Authorization
and rate admission happen before waiting for a per-location mutation lock or
filesystem probing. Reserved
files, symlinks and cross-device entries are omitted. Download selections are capped
at 500 top-level items, 100,000 entries and 64 levels; symlinks, special files,
non-Unicode names and inaccessible entries are skipped and reported to the client.
Staged file/directory collisions, including case and Unicode normalization twins,
are skipped per item without aborting the download. Multi-select trash reports
each item's destination or error, continues after failures, and retries only
failed selections. Locations polling contacts only friends currently online in
the existing presence store. Upload chat completion records use landed NAS file
and directory paths; private staging callbacks cannot mark chat files complete.

Each location has a configurable byte cap (Settings, GB), default **20,000,000,000
bytes** per upload stage or download snapshot. Upload manifests are checked before
receiving, including retained retry bytes plus the prospective incoming copy,
with actual sizes checked again before publication. A retry near the cap may need
to wait for orphan GC or a host cap increase. The cap covers file payload; small
engine sidecars and directory metadata also use disk space. Snapshots preflight
the complete byte total and enforce the cap while copying, including growing files.
`statvfs` checks local staging/snapshot free space and NAS publication space, with
64 MiB headroom. This is a precheck, not a disk reservation against other processes.
At most two upload/download preparations/transfers per friend can hold leases.

Cache GC runs at startup and every 30 minutes, only in config/location-snapshots
and config/location-transfers. Generated, inactive directories are orphaned and
removed (including ones older than 24 hours); active leases are always protected.
GC snapshots active leases, releases the registry mutex, then claims each orphan
under a short lock before deleting it. Errors are logged per entry and do not
stop the sweep. No NAS paths or recoverable trash are swept. Ordinary receives
register their destination before probing. Existing startup/cache-clear sweepers
clean empty `.dropbeam-probe-<uuid>` files older than 24 hours in those directories,
excluding Locations and their descendants. Recent files, symlinks, nonempty
files and non-generated names are preserved. Failed/canceled upload staging can
resume until the next sweep. Snapshot destruction schedules recursive deletion on
`spawn_blocking` (a separate thread without a Tokio runtime). Upload preparation
and finalization also run on blocking workers in both GUI and headless receive.

## Validation and limits

The tests in `locations.rs` cover path traversal/absolute/Unicode handling,
symlink escapes (including a poisoned trash path), root protection, recoverable
trash, no-replace rename/publication, SHA mismatch, ACL/right checks, revocation,
pagination, private snapshots and corrupt-config preservation. The iroh loopback
test exercises actual capability negotiation, listing, Unicode upload, folder and
empty-directory upload, the normal 16 MiB resumable path, and permission denial.
It uses loopback-only endpoints with relay and discovery disabled.

Phase 1 hosts are **Linux and macOS**. Windows and phone clients can use a supported
host; secure Windows hosting is not implemented and fails closed. The UI was built,
but native mobile device and actual Ubuntu GVFS/NAS compatibility were not exercised
in this environment. The forced-reservation test covers the fallback on the local
filesystem; it does not establish behavior on the actual family NAS.

Downloads snapshot selections privately on the host before starting the friend
send, and uploads stage on the host before copying to the NAS. This costs extra
local disk space and I/O; large snapshots show “Preparing” in the browser. Interrupted
uploads retain private staging for retries until GC. The existing engine prepares a
complete manifest, hashes paths and reopens them on retries, so this change retains
bounded private snapshots rather than streaming a changing NAS tree. No filesystem
watcher, trash restore UI, streaming snapshots, or Windows host adapter is included.

## Verification in this checkout

Review-hardening validation is reported with the final change summary. Tests include
forced reservation file/directory moves and collisions, byte caps/free-space refusal,
protected roots, mirror overlap, created modes, mount pins, cursor consistency/rate
limits, concurrency/GC, skip reporting, Unix colon/backslash names and a paused
upload copy that permits listing/settings save but refuses revoked publication.

Second-review validation on the macOS development host is recorded in the final
change report. The targeted suite includes marker-matched remount re-stamping,
32-file upload probe counts after cache eviction, read-only and unknown probe
errors, file/directory staging collisions, case/normalization twins, ordinary
receive probe caching/cleanup, clock-injected token buckets for all rate-limited
routes, total cursor-entry bounds, recursive Library protection, session mount
replacement, and explicit marker retirement. The previous safety, permissions,
revocation, trash and integrity tests remain in place.

Final second-review validation:

- `npm run build`: passed; existing bundle-size warning remains.
- `cargo build --release --lib --offline`: passed.
- `cargo test --offline`: 181 passed, 6 ignored, 40 failed. Every failure was
  a loopback socket-bind denial (`Operation not permitted`, OS error 1) in the
  sandbox. All 28 Locations tests passed.
- `git diff --check`: passed. No version fields changed; no commit or push.
