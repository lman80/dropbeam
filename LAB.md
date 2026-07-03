# DropBeam Lab — two-machine transfer test driver

A dev-only CLI (`src-tauri/src/bin/dropbeam-lab.rs` + `src-tauri/src/labkit.rs`)
that runs REAL engine transfers (`send_files` / `recv_files_negotiated`, the
production `presets::N0` endpoint, the app ALPN) between two processes or two
machines, and prints JSON lines an automated runner can verify. It is never
bundled into the app users install.

## Roles

**Receiver** (the second machine — zero setup beyond double-clicking):

```
dropbeam-lab serve [--dest <dir>]
```

Prints `{"event":"ready","addr":"lab…"}` then one `received` line per inbound
transfer (file count, bytes, ms, per-file sha256s, whether the parallel path
engaged). `Start DropBeam Lab.command` wraps this for non-technical use: it
copies the lab code to the clipboard and shows friendly progress.

**Sender** (driven by the runner on the first machine):

```
dropbeam-lab send --to <labADDR> [--mode auto|direct|relay]
                  [--suite quick|full|big] [--parallel on|off] [--dir <corpus>]
```

Builds a deterministic corpus and pushes each case over a fresh connection:
single 1 MiB, 60 small files, unicode/odd/empty names, a nested folder tree
(with an empty dir), plus a 256 MiB file (`full`) and 1 GiB (`big`). Each
`sent` line carries the expected sha256s + throughput.

**Results pull** (how the runner reads the second machine without SSH):

```
dropbeam-lab results --to <labADDR>
```

Dials the receiver on a side ALPN (`dropbeam-lab/results`) and prints its
accumulated results as a JSON array. Verification = compare the sender's
expected hashes with the pulled receiver hashes, in connection order.

## Dial modes

The peer's advertised addresses are FILTERED before dialing: `direct` keeps
only IP addrs, `relay` keeps only the relay URL (forces the initial dial + early
bytes through the relay even on the same LAN), `auto` is production behavior.
Note iroh may still hole-punch an upgrade mid-transfer in relay mode — exactly
like the shipping app; the first-case throughput is the relay-path signal.

## Invariants

- The lab reuses engine functions verbatim — never fork transfer logic into it.
- `labkit::lab_endpoint` must stay on the production preset so results transfer
  to the real app.
- The corpus generator is deterministic (`payload(len, seed)`), so both sides
  can be compared byte-for-byte by hash without shipping the files twice.
