# Transfer integrity v1

Friends, Quick Send, and Locations file transfers negotiate end-to-end integrity.
Both devices independently hash file bytes and retain the resulting comparison in
transfer events and History. Chat cards aggregate results across split pushes and
persist them across reloads. A completed transfer from an older peer still says
Saved/Delivered; it does not acquire a Verified badge.

## Digest definition

For file size `S`, block size `B = 4,194,304`, and consecutive blocks `b[i]`:

```
leaf[i] = SHA256(b[i])
root = SHA256("DropBeam integrity v1\0" || u64be(S) || u64be(B)
              || leaf[0] || leaf[1] || ...)
```

The prefix is the literal ASCII text followed by one NUL byte. Leaf values are
32 raw bytes, not their hex representation. The root is lowercase hexadecimal.
The final block may be short; an empty file has no leaves. There is no leaf for
an empty trailing block. The displayed algorithm is
`SHA-256 / DropBeam blocks v1 (4 MiB)`.

This is a file fingerprint using SHA-256, not the output of plain `sha256sum`.
The canonical boundaries make it independent of network chunk sizes, parallel
stream order, or resume layout. File size is bound into the root. Tests compare
the accelerated implementation against an independent `sha2` implementation.

`ring`, already locked and cached through TLS, supplies hardware-accelerated
streaming SHA-256. No new transitive dependency or package version is introduced.
The existing `sha2` implementation hashes the small root input.

## Streaming and resume

Classic send/receive paths hash buffers as they read/write. Parallel workers hash
fixed, disjoint blocks as they transfer each range. Only block digests are kept
(32 bytes per 4 MiB, plus map bookkeeping). A fresh transfer adds no file readback.
Classic receives use exclusively created staging files and publish without replacing an
occupied name (including a dangling symlink). Parallel resumes verify before
publishing. Ordinary publication always attempts native exclusive rename, then atomic hard-link
publication when unsupported. Only if both are unsupported does it use the logged
reservation fallback, with the concurrent-writer limitation documented for Locations.
Collision retries advance one bounded candidate sequence and report exhaustion.
Classic stages are registered before creation, removed on failure, and swept in
registered destination directories (including nested directories) at startup
and hourly. Live stages are excluded from cleanup.

A resumed attempt retains only complete canonical blocks (including the short
last block when fully landed); partial blocks are retransmitted. Both devices
reread the retained ranges concurrently with transfer of the missing ranges.
Sidecars remain compatible: a prior sidecar cannot certify content, so cached
coverage is never used as a substitute for independently checking retained bytes.
Incoming integrity ranges must be aligned, in bounds, and non-overlapping.
Advancing rehash bytes refresh local transport activity and travel in receiver
progress frames. An independent task activity counter also feeds the chat watchdog,
even when displayed-byte totals are unchanged. The partial and sidecar remain
available through rehash. Before verification, old coverage is revoked. A root
mismatch retains only blocks whose digests match the peer’s root-bound leaf list;
writing and renaming the invalidated sidecar must succeed. A failed save leaves
no reusable coverage, so the next attempt starts clean. Successful verification
finalizes the file.

## Wire negotiation

- The `files` header advertises `integrity_v: 1`.
- A receiver echoes `integrity_v: 1` in its existing `ready` frame.
- After all payload bytes/ranges, the sender sends `integrity_blocks` pages with
  index/offset/leaf digest rows, followed by digest pages on the original
  bidirectional stream: `{"kind":"integrity","integrity_v":1,"files":[...],"more":true}`.
  The last page sets `more:false`; each frame is at most 256 KiB, below the 1 MiB read cap.
- Receipt pages use `kind:"integrity_receipt"`, an `integrity` array, and the same
  continuation flag. The terminal success/error frame follows with `integrity_done:true`.
  The sender validates all pages against its own roots before sending
  `{"kind":"integrity_ack"}` and FIN. Receiver History/UI reports Verified only after
  reading that acknowledgement. Lost receipt/acknowledgement leaves Saved, unverified.
- Rows include the original manifest `index` plus relative `name`, size, algorithm,
  local/peer digests, checksum-match `verified`, and receipt `acknowledged` status.
  Index/name identity survives split pushes and retries; duplicate leaf names remain distinct.
- Quick Send advertises capability in its initial `pull` request too. Only a
  capable request enables the new header/receipt exchange. Its established raw
  trailing `ok` remains after the integrity terminal receipt.
- Classic integrity negotiation runs alongside body writes, with a six-second
  deadline. Whichever arrives first, the deadline or completed body, can select
  unverified mode. The sender immediately FINs without any integrity trailer.
  A later integrity echo cannot change that decision; clean EOF before any
  extension means unverified. EOF after an extension starts is an error. Legacy
  headers carry no integrity indices; negotiated digest rows carry those indices.
  Chat’s optional manifest metadata carries its own starting item index. A ready reply already known to omit integrity
  skips hashing and integrity extensions entirely. Legacy one-way receive helpers
  explicitly ignore the advertisement.
- Empty `more:true` pages are rejected. Digest and receipt reads share the
  60-second inactivity budget, refreshed only by advancing hashing, byte, or page
  work; unchanged progress ticks do not extend it. This covers friend and Quick Send paths.
- Locations retains its existing publication/access checks and SHA-256 checks;
  its terminal receipt follows successful publication.

A mismatch keeps the received file (or resumable partial plus sidecar) and reports `Verification failed — retry` on
both sides. It does not automatically resend a finalized corrupt file. Mismatch
logs include both sizes, both digests, and the algorithm, with names/paths omitted.
History retains mismatch results. The UI exposes selectable local and peer digests
through the Verified/checksum details on transfer cards, chat cards, and History.

## Validation

The integrity tests cover independent combine vectors, read/stream order,
canonical resume alignment, invalid receipts, duplicate filenames, single/multiple/empty files,
parallel transfer, sparse persisted resume, corruption of a retained landed
block, and Quick Send. Existing regressions cover old peers, delayed capability
echoes, landed progress, empty directories, Locations publication and permissions,
chat attempt ordering, and partial cleanup.

Run with cached dependencies and localhost socket permission:

```
npm run build
node --experimental-strip-types --test tests/*.test.ts
cd src-tauri
cargo build --release --lib --offline
cargo test --offline -- --test-threads=1
cargo test --offline --release --lib integrity_loopback_overhead -- --nocapture --test-threads=1
```

The overhead test alternates three 32 MiB loopback sends to an integrity-capable
receiver and a receiver omitting the integrity echo. Timing includes payload,
flush/finalization, digest exchange, and receipt; it excludes fixture creation
and test readback. Both peers share this Mac's CPU and filesystem. Debug timings
are not representative of the shipped optimized build.

## Files for this feature

- `src-tauri/src/iroh_net/integrity.rs`: block hashing, resume plans, comparison,
  receipt validation, task-scoped results, and combine tests.
- `src-tauri/src/iroh_net.rs`: streaming and protocol integration, reporting,
  mismatch handling, and loopback/compatibility tests.
- `src-tauri/src/models.rs`: per-file integrity records in events and History.
- `src-tauri/src/sync.rs`: backward-compatible empty integrity field in existing
  shared-folder History construction.
- `src-tauri/Cargo.toml`, `src-tauri/Cargo.lock`: direct use of the already-locked
  `ring` dependency; existing package versions are unchanged.
- `src/lib/api.ts`, `src/lib/integrity.ts`, `src/lib/normalize.ts`: integrity types
  and normalization.
- `src/lib/chatTransfer.ts`, `tests/chat-transfer.test.ts`: split-batch aggregation,
  persistence, retry isolation, and tests.
- `src/components/IntegrityDetails.tsx`, `src/components/TransferCard.tsx`,
  `src/components/ChatTransferProgress.tsx`, `src/views/HistoryView.tsx`: shared
  check mark and selectable digest details.
- `docs/transfer-integrity.md`: this protocol and validation reference.

The existing Locations implementation, chat-progress/attempt fixes, and unrelated
documentation are preserved. This work makes no commit or push and changes no
application version field.

## Measured overhead

On this Mac, the optimized loopback benchmark with hardware-accelerated SHA-256
measured **0.729 s without integrity vs 0.845 s with integrity** for three 32 MiB
transfers: **15.9%**, or approximately **39 ms per 32 MiB transfer**. Verified
throughput was **113.6 MiB/s** in that run. This includes two endpoints sharing
one machine; it is not a prediction for a particular LAN, drive, or file mix.
The initial software SHA-256 build measured 160.1% overhead in the same test,
which motivated using the existing hardware-accelerated crypto backend.

The review regressions additionally cover both late-echo/body-FIN orders, timely
legacy EOF, a 5,000-file paged digest/receipt exchange, unacknowledged receipts,
256 MiB retained rehash under a 700 ms watchdog, destination and dangling symlinks,
and successive duplicate-name splits plus retry (Rust and TypeScript).

Review-fix validation: `npm run build` and
`cargo build --release --lib --offline` passed. The offline Rust suite with
`-- --test-threads=1` passed **210 tests**, with **6 existing tests ignored**;
binary and doc-test harnesses also passed. Localhost socket access was required
after the sandbox denied loopback binds. All **16 TypeScript tests** passed.
After a resume root mismatch, the bytes and sidecar are retained but suspect
coverage is reduced to matching blocks, so an explicit Retry retransmits only
mismatching blocks. Round 2 regressions also exercise the production friend chat
admission and indexed landed-path lookup with duplicate names, short chat
watchdog liveness during rehash, paging inactivity and empty continuations,
failed invalidation persistence, bounded collisions, and classic-stage cleanup.

Round 2 validation: `npm run build` and
`cargo build --release --lib --offline` passed. The full offline Rust suite
passed **227 tests** with **6 existing tests ignored**, using localhost socket
permission and `--test-threads=1`. After adding the hourly sweep schedule, the
release library was rebuilt and all **3 cleanup regressions** passed again.
All **17 TypeScript tests** passed, including duplicate-name completed-path
lookup after reload. No application version fields were changed.
