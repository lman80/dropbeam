# Release-candidate P1 regression audit — 2026-09-09

Scope: BUG-23 and §6g/BUG-24 in the supplied feature-sweep report. Reviewed the supplied crash report and the `6d017b9..HEAD` additions to `iroh_net.rs`, `sync.rs`, `commands.rs`, and `friends.rs`, including their receive/progress, presence, Verify, picker, clipboard, and friend restoration paths. Work was offline, without subagents, commits, or pushes.

## BUG-23: findings and changes

The sole application panic hook was in `lib.rs`; it unconditionally called `log::error!`. It had no stderr fallback and could re-enter the logging system while a logger lock was held or poisoned. The supplied stripped crash stack supports a nested panic/abort, but does not identify the original panic or prove which logger operation caused the second one.

The new `panic_log.rs` hook wraps its body in `catch_unwind(AssertUnwindSafe(...))`, formats only built-in strings and integers into a bounded 4 KiB stack buffer, and writes directly to the OS stderr handle and a pre-opened, append-only `DropBeam-panic.log`. It does not invoke the normal logger, payload formatters, or application/stdio mutexes. Errors are ignored. This design matters because a nested panic inside a panic hook can abort before `catch_unwind` gets control. The panic file is excluded from log pruning, and telemetry uses its modification time for digest inclusion outside panic handling.

A separate, reproducible panic candidate was found in telemetry: `signature()` truncated arbitrary UTF-8 at byte 120. It now truncates at a character boundary. A regression test covers a multibyte character crossing that boundary. This is a confirmed defect, not a proven explanation of the reported abort.

The Rust audit also made the following paths fallible:

- Presence checks closure before and after sampling received datagrams; both dispatcher exit paths remove only the matching cached connection. Missing friends already resolve through `Option` and emit nothing.
- Optional capability/connection caches and the progress throttle no longer unwrap poisoned locks. Capability persistence uses fallible thread creation.
- Progress output handles a missing terminal sender as cancellation/error. Progress-reader EOF, malformed frames, invalid counts, and incomplete terminal receipts remain errors.
- Byte totals use checked addition; incoming classic manifests must agree with their totals. Parallel paths use a fallible first-file lookup; generic payload writes validate the returned count before slicing.
- Verify returns an unavailable comparison on poisoned state; reply dispatch tolerates missing/poisoned handles. Viewer warning emission releases its mutex before emitting.
- Friend mutations recover the serialization-only `Mutex<()>` and reload disk state. Missing friends/threads already use guarded lookups, retain, or optional removal; restoration tests pass. Existing vector indices derived from the same unmodified local vector, fixed-size frame slices, and immutable JSON indexing were checked and are bounded/non-panicking under these conditions.
- Clipboard RGBA dimensions are checked before invoking the PNG encoder, which requires a matching buffer length.

There is no application `remote_map` lookup. The cached iroh 0.98.2 implementation delegates `Connection::stats()` to noq's retained connection state; `paths()` retains its watcher source. No closed-connection-specific panic was found there. The close-during-refresh loopback test passes, but cannot exclude every transport-internal race.

## BUG-24: findings and changes

The new native picker used `NSOpenPanel.runModal()` inside Tauri main-thread dispatch, nesting an application-modal event loop. This is the strongest code-level explanation for the reported UI freeze. SendView and ChatView previously had no picker busy state; Popover already cleared its state in `finally`. No cancel-only overlay leak was found in these components.

The picker now presents an asynchronous sheet on the visible main window, with a modeless completion-based fallback when that window is hidden/unavailable. AppKit's main-thread completion handles selection, cancellation, and failure; it orders the panel out, resolves the oneshot, and takes/drops the captured panel ownership to break the retain cycle. A process-wide guard prevents simultaneous panels. SendView/ChatView now clear their picker state in `finally`; DropZone disables only its own button while picking; Popover guards both picker entry points. Transfer-list rendering is independent of picker state.

## Files changed

- `src-tauri/src/panic_log.rs` (new), `lib.rs`, `telemetry.rs`: emergency diagnostics and UTF-8 fix.
- `src-tauri/src/iroh_net.rs`, `sync.rs`, `friends.rs`: audited fallible paths and regression coverage.
- `src-tauri/src/commands.rs`: asynchronous native picker and clipboard validation.
- `src/views/SendView.tsx`, `src/views/ChatView.tsx`, `src/components/DropZone.tsx`, `src/windows/Popover.tsx`: picker lifecycle/entry guards.
- `src-tauri/Cargo.toml`: direct dependency on already-cached block2; release `debug = 1`, `split-debuginfo = "packed"`, `strip = false`, with an archival comment.
- `src-tauri/Cargo.lock`: only the application's block2 dependency entry was added relative to the initial workspace.
- This audit note.

All version fields were compared with an initial workspace snapshot and are unchanged. `src-tauri/tauri.conf.json` is byte-for-byte unchanged. The three config files already contained uncommitted version changes before this task; those were preserved.

## Validation and limits

- `npm run build`: passed (existing chunk-size advisory).
- `cargo build --release --lib --offline`: passed, optimized plus debug information. A packed `libapp_lib.dylib.dSYM` was generated; its UUID matches the built library and it contains line tables.
- `cargo test --offline`: passed with local socket access: **165 passed, 0 failed, 6 ignored**, plus main/doc tests. The initial sandbox run had 26 failures, all at loopback socket binding (`Operation not permitted`). A separate offline loopback run passed all 30 tests.
- New coverage includes eight close-during-presence iterations, dispatcher cleanup, malformed/interrupted landed progress, byte-total overflow, UTF-8 truncation, and a subprocess panic hook test with a broken logger, held stderr lock, poisoned mutex, non-string payload, and oversized payload. Existing friend removal/restoration tests passed.
- `git diff --check`: passed. Existing compiler dead-code/unused-import warnings remain.

The live macOS picker Cancel/navigation/incoming-card sequence and prolonged idle soak were not reproduced. The original abort's triggering panic remains unidentified. The requested library build does not rebuild/rebundle the application executable; the matching executable dSYM must be retained when the release app is subsequently built. New symbols cannot symbolicate the old stripped build.

Build-space cleanup removed disposable Cargo incremental/old release caches; the prior application binary and bundle were retained. No app data was changed.
