# Cross-machine transfer tests

Real-app and engine tests across several machines (used 2026-09-25: this Mac,
a second Mac on the same Wi-Fi, and a Linux box in the US reached over Tailscale).

- `run.sh <label> <addrfile> <mode> <suite> [--only case] [--parallel off]` — engine
  suites via `dropbeam-lab` (`cargo build --release -p dropbeam-lab`; receivers run
  `dropbeam-lab serve`). Suites: quick, full, big, huge, sized (LAB_SIZE_MIB), edge,
  many, mixed, torture2. Modes: auto, direct, relay.
- `realapp.py "<src>>dst,…" <kinds> <ops>` — drives the REAL apps through the lab-mode
  automation queue (`automation-queue.json`, see src-tauri/src/automation.rs) and
  sha256-verifies what lands. Needs Lab Mode on (settings `labModeEnabled`) on every
  machine. Kinds: photo, multi, folder, many, video, big. Ops: send, quicksend.
- `overnight.sh` (HOURS=4) — loops both, logging to overnight.log / realapp.log.

macOS gotcha: a freshly re-signed build triggers the Downloads/Desktop privacy prompt;
on an unattended Mac point `downloadDir` at a non-protected folder for testing.
