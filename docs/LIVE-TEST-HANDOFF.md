# Live-test handoff (fix-live worktree, 2026-10-06)

Branch `fix-live` at `~/DropBeam-wt/fix-live` on ashton-mac. `ios` (with ux-accounts + ux-everyday) merged in at `43db9eb`.
All three machines run **449ddd9** (v0.53.0 QA build, `--features lab`).

Machines (lab names in `~/DropBeam-wt/nightly/realapp.py`):
- `m1` = this Mac (ssh `ashton-mac`), in Korea
- `mac2` = Mong (ssh `minkyung-mac`, old alias `mac2`)
- `lin` = Linux box (ssh `linux-agent`, old alias `penis`), US. So m1↔lin is **internet**: direct p2p, RTT about 200 ms.

## Test matrix

| Area | Pair | Status |
|---|---|---|
| Chat offline queue (edit/unsend/order/delivered) | m1→mac2 | PASS (`chat2.py`) |
| Edit file mid-send (200 MB @3 s, 300 MB @12 s / 200 MB @6 s) | mac2→m1, m1→mac2 | PASS (`editsend.py`) |
| Folder basics: add, edit, same-size edit, delete, rename, case-only rename | m1↔mac2 | PASS (`fold1.py`) |
| Folder basics (same) | m1↔lin (internet) | PASS on L2 and L3 after fix 6dfac65. Before it, delete took more than 180 s. |
| fold2 `hist` (delete of settled file → History on B → restore) | m1↔lin | PASS on 43db9eb. **On 449ddd9 "delete of settled file" FAILED (204 s); History entry PASS.** Not analysed yet (see Open). |
| fold2 `closed` (delete while app closed) | m1↔lin | FAILED before 449ddd9 (root cause fixed there). **Not yet verified on 449ddd9.** |
| fold2 `moveroot` (root moved away → no mass delete; restore converges) | m1↔lin | PASS |
| fold2 `slow` (200 MB slow copy-in lands whole) | m1↔lin | FAIL by timeout only. The link ran 0.2 MB/s (scp over Tailscale was even slower), so the transfer takes about 19 min against a 300 s test timeout. No truncated copy was ever seen. Not an app bug; rerun on LAN or raise the timeout. |
| fold2 on LAN | m1↔mac2 | not yet run |
| Friend requests / block / unblock / remove (via `~/qa-stranger.sh` second identity on Linux) | — | not yet run. Script written, ops added (b55c5c9). |
| iOS, Windows | — | not run in this session |

## In flight when stopped
L3 pair (`/tmp/fold-L3.json`, folders `~/DropBeamQA-Shared-L3` on m1 and lin) is in the "stale mtime" state. Linux held `closed/x.txt` with the same bytes but an older mtime. That was the 449ddd9 bug.

Next steps on 449ddd9:
1. `python3 fold2.py L3 closed`
2. `python3 fold2.py L3 hist`. Find out why "delete of settled file" failed.
3. `python3 fold1.py m1 lin L4`

## Bugs found and fixed
- **6dfac65**: a peer delete refused as "freshly added" (2-min grace) only landed on the next idle beacon, 5+ min later. Now one coalesced re-check per folder fires when the window closes.
- **b55c5c9**: lab automation gets `unblock` and `remove-friend` ops (test cleanup).
- **2b13030**: crash-orphaned `<name>.dropbeam-incoming` placeholders were never swept while a folder was busy. The idle rescan is starved because every beacon's `wake_sender` restarts its sleep. `live_manifest` (run every control round) now sweeps stale placeholders too.
- **55e4bdf**: a versioned tombstone for a version we don't hold left the folders split forever (delete refused, push blocked by the newer tombstone). Such versions are now dropped from the delete plan and pushed if the peer lacks the file.
- **449ddd9**: a same-size, mtime-only mismatch (sync never re-sends those) blocked deletes of identical files, and after 55e4bdf it resurrected them. `delete_allowed` now treats same size + different mtime as the in-sync file unless ours was placed after the delete. Survivors are exactly what `delete_allowed` keeps.
- Test-script fixes (nightly, not in repo): `chat2.py` uses `pkill … ; true`; `fold2.py` uses `wc -c` instead of `stat -f` (Linux).

## Open / suspected (not fixed)
- `hist` "delete of settled file" failed once on 449ddd9 (204 s). It could be a regression from 449ddd9's `delete_allowed` change or a link stall. Check this first. Code review (2026-10-07): 449ddd9 is unlikely to be the cause. The reconcile `plan.delete` only lists rels whose peer tombstone is newer than the file, so `version_mismatch_survivors` keeps none of them, and the new same-size branch only makes `delete_allowed` more permissive. Rerun live to tell a link stall from something else.
- Once, Linux's lab endpoint stopped answering ("dial device app endpoint: timed out") while the GUI was still active. A restart fixed it; the cause is unknown.
- The idle rescan (`seed_existing`, sync.rs ~1216) is starved while the peer is online, because `wake_sender` on every control success restarts its 45–180 s sleep. 2b13030 only moved the placeholder sweep. Other rescan duties may also be starved.
- Stale partial on Linux: `~/.config/com.dropbeam.app/folder-partials/.dropbeam-partial-ac541896e83a3a9e.*` (200 MB, from the morning). Check that partials are GC'd.
- ~~Flaky `xfer_matrix::matrix_resume_*`~~ fixed in bd98f38 (2026-10-07): with 16 MiB lanes the "interrupted" send could finish from buffered flow-control windows (Windows CI: a complete, verified `movie.mov` at its real name, not a truncated one). The test now uses 32 MiB lanes and breaks at half. Not an engine bug.
- m1 logs at "full (app+iroh)" verbosity: 4 MB rotation about every 20 min, so app_lib lines get lost. Use app-level verbosity for tests.

## Rebuild + deploy
Mac (builds into `~/DropBeam-ios/src-tauri/target`; disk is tight, so delete a `target/` if a build runs out of space):
```
bash ~/DropBeam-wt/qa-mac-build.sh        # log: ~/DropBeam-wt/qa-mac-build.log, prints BUILT
bash ~/DropBeam-wt/qa-install.sh m1 mac2  # installs the QA zip on both Macs, relaunches
```
Linux (about 15–25 min). Wait for the `EXIT` line before running dpkg, or you reinstall the old .deb:
```
cd ~/DropBeam-wt/fix-live && rsync -az --delete --exclude node_modules --exclude dist --exclude target --exclude .git --exclude 'src-tauri/gen/apple' ./ linux-agent:~/DropBeam/
ssh linux-agent 'cd ~/DropBeam && (source ~/.cargo/env; nohup sh -c "npx tauri build --bundles deb --features lab > /tmp/lin-build.log 2>&1; echo EXIT \$? >> /tmp/lin-build.log" >/dev/null 2>&1 &)'
# when grep -q "^EXIT" /tmp/lin-build.log succeeds (EXIT 1 = updater-signing key missing, harmless; check the .deb timestamp):
ssh linux-agent 'sudo dpkg -i ~/DropBeam/src-tauri/target/release/bundle/deb/DropBeam_0.53.0_amd64.deb && systemctl --user restart dropbeam-gui'
```
Unit tests: `cd src-tauri; CARGO_TARGET_DIR=~/DropBeam-ios/src-tauri/target CARGO_INCREMENTAL=0 cargo test --features lab --lib` (576 pass on 449ddd9).

## Driving tests (`~/DropBeam-wt/nightly`, on ashton-mac)
- `python3 fold1.py A B [TAG]`: creates pair `DropBeamQA-Shared-TAG` and writes `/tmp/fold-TAG.json`. Runs add/edit/delete/rename/case tests.
- `python3 fold2.py TAG slow,hist,closed,moveroot`
- `python3 chat2.py m1 mac2`, `python3 editsend.py FROM TO SIZE_MB EDIT_AT_S`
- `from lab import *`: `lab(m, op, **kw)` with ops `ping`, `fs-manifest`, `fs-write` (pairId, rel, size, seed), `fs-delete`, `history-list`, `history-restore`, `pair-dump`, `reconcile`. Also `manifest(m, pairId)` and `converge(...)`. Note that `converge` needs exactly equal manifests, so any stray `.dropbeam-incoming` fails it.
- Real-app automation queue (Lab Mode): JSON array in `<config>/automation-queue.json`. Ops: `send`, `send-nonote`, `quicksend`, `receive`, `chat`, `chat-edit|unsend|react|reply|read|typing`, `addfriend`, `accept-request`, `block`, `unblock`, `remove-friend`, `cancel`. Results go to `automation-results.jsonl`.
- Logs: Mac `~/Library/Logs/com.dropbeam.app/`, Linux `~/.local/share/com.dropbeam.app/logs/` (timestamps UTC).

## Left changed on machines (restore when done)
- All 3 run **QA builds** (`--features lab`) of fix-live, not a release. Linux has `labModeEnabled: true` with a `labOperatorId`.
- Linux `~/DropBeam` source tree is overwritten by the fix-live rsync.
- Linux `~/qa-stranger.sh` (second identity under `/tmp/qa-stranger`): written, never started.
- Test folders/pairs `DropBeamQA-Shared-{L2,L3,f1,w1,…}` on m1/mac2/lin. Remove the pairs and folders.
- m1 verbose diagnostics "full (app+iroh)". It's unclear whether the tests set this; check that it matches the owner's preference.
- Untracked `src-tauri/gen/apple/assets/` was moved to `/tmp/fixlive-apple-assets-bak` for the merge (now tracked from ios).
- ~1 GB cleared from `~/dbtest-src/*` (test scratch) for disk space.
