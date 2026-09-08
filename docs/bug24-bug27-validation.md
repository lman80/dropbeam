# BUG-24 / BUG-27 validation — 2026-09-09

## BUG-24: findings and limits

Read sections 6–7 of the supplied feature sweep. Ran `npm run dev -- --host
127.0.0.1` and drove the app through Playwright MCP. Mock mode is selected by
the absence of `window.__TAURI_INTERNALS__`; this checkout does not need a
`VITE_MOCK` variable. Mock picker cancellation was an empty array, matching the
native API contract. No native Mac picker or WebKit reproduction was performed.

The following exact exceptions were reproduced **before the fix**, by injecting
incomplete payloads into the frontend store:

| Payload / action | Exception | Throw site |
| --- | --- | --- |
| Chat message without `files`, then search for nonmatching text | `TypeError: Cannot read properties of undefined (reading 'some')` | `Conversation`, `ChatView.tsx` search filter |
| Text message without `text` | `TypeError: Cannot read properties of undefined (reading 'matchAll')` | `Linkified`, `ChatView.tsx` |
| Incoming transfer without `fileNames` | `TypeError: Cannot read properties of undefined (reading 'length')` | `title`, `TransferCard.tsx` |

Each exception unmounted the **entire React root**, including the sidebar. Thus
these are demonstrated crash paths, but **not a confirmed cause of the original
Mac wedge**, where the sidebar continued updating. This checkout had no React
boundaries that could have let a sidebar survive a content render exception.

The normal Copy code action, mocked picker cancellation/error, clipboard failure,
and success/info/error toasts did not produce an uncaught exception or block
navigation. Successful Copy code uses local `Copied` button state, not a toast;
copy failure shows a toast. Zero totals, missing speed, landed zero, NaN and
Infinity did not throw through ETA/formatting, but NaN/Infinity leaked into labels.
Undefined avatar sources, the send chooser and the viewer banner also rendered.
These browser results do not exclude native input or animation/compositing faults
in the reported Mac build.

### Changes and verification

- Normalize live and loaded chat messages before storing them: missing text,
  files, reactions and numeric fields receive safe defaults. Search, previews,
  file bubbles and link rendering consume the normalized data.
- Normalize transfer counters, percentage, ETA and filenames at ingestion;
  retain known filenames on sparse updates. Formatters reject non-finite values.
  Avatar initials and source handling tolerate absent/non-string data.
- Independent boundaries protect sidebar, keyed page content, toasts, overlays
  and window controls. A root boundary also covers initialization rendering and
  the popover/HUD/receive window roots. Navigation remounts a failed content
  boundary. Fallbacks have no store or animation dependency and display
  “Something went wrong — reload” with a Reload button.
- `componentDidCatch` records the region, error stack and React component stack
  through `api.frontendLog` / `frontend_log`. Rust logs these `[ui]` messages at
  ERROR severity so they enter the existing diagnostics digest. Bridge failures
  are caught without involving toasts.
- Post-fix Playwright injected sparse `transfer://update` and `chat://message`
  events through the mock event bus, plus a sparse stored chat message. All
  rendered and searched without uncaught exceptions or non-finite labels.
- Deliberately bypassed normalization to force content, sidebar, toast and
  overlay errors. Each displayed its own fallback and submitted a diagnostic
  message. Content/toast/overlay failures left navigation clickable; sidebar
  failure left the picker clickable. A rejected logging promise caused no
  additional uncaught error. Reload recovered the app.

Boundaries contain React render/lifecycle errors; they do not recover a native
input deadlock or catch arbitrary asynchronous callback failures.

## BUG-27: receipt contract

The production Quick Send sender consumes a trailing raw `ok`. Previously the
pull receiver reused the push receive path: a header carrying `progress_v` made
it enter framed progress mode and suppress the raw receipt. The sender could
therefore report unconfirmed delivery despite byte-identical saved files.
The sender in **this checkout already passes `progress=false`** to its header
builder; the incompatible advertised-header case is covered explicitly as a
compatibility regression, not claimed as a fresh two-current-build reproduction.

`read_pull_files_negotiated` now removes the push-only `progress_v` capability
before negotiation, retains parallel/resume support, and writes raw `ok` only
after successful receive. It finishes and drains the receipt before letting the
short-lived pull connection drop. Previously the receive path returned immediately
after enqueueing the receipt, which also allowed a connection-close race.
The production receive command and ticket helper share this implementation.
Push receive behavior is unchanged.

Two offline loopback tests use real tickets and the production pull functions:
`quick_send_code_confirms_delivery` and
`quick_send_progress_header_still_gets_raw_receipt`. Each covers zero-byte,
1 KiB and parallel-sized files. They assert byte equality **and exact `ok` on
the sender**, including older headers that advertise progress. The sender
endpoint remains alive after each serve task, as it does in the app.

## Validation

- `npm run build`: passed (existing bundle-size warning).
- `node --test tests/*.test.ts`: 6 passed.
- Targeted ESLint on the new boundary/normalization/tests and edited formatting/
  avatar files: passed. `git diff --check`: passed.
- `cd src-tauri && cargo build --release --lib --offline`: passed, with unused
  import/dead-code warnings.
- `cargo test --offline` in the sandbox: 139 passed, 28 loopback failures, 6
  ignored; every failure was `Failed to bind sockets: Operation not permitted`.
- Same command with approved localhost socket access: **167 passed, 0 failed,
  6 ignored**. Binary and doc-test targets also passed. No relay/discovery tests
  were enabled and no dependencies were downloaded.

## Files changed by this task

- `src/App.tsx`, `src/main.tsx`, `src/components/ErrorBoundary.tsx`: isolation and recovery.
- `src/store.ts`, `src/lib/normalize.ts`: defensive ingestion of transfer/chat data.
- `src/lib/format.ts`, `src/lib/avatar.ts`, `src/components/FriendAvatar.tsx`: safe display values.
- `src/lib/mock.ts`: export the existing event emitter for browser regressions.
- `src-tauri/src/lib.rs`: diagnostic severity for caught render errors.
- `src-tauri/src/iroh_net.rs`: pull receipt handling and loopback regressions.
- `tests/ui-data.test.ts`: missing-field and numeric regressions.
- `docs/bug24-bug27-validation.md`: this report.

The pre-existing modifications to `Cargo.toml`, `Cargo.lock` and
`tauri.conf.json` were left intact; their file hashes were unchanged across
validation. No subagents, commits or pushes were used.
