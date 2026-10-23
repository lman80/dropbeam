# Network lab — 2026-10-23

Goal: get DropBeam to the fastest speed the devices' own upload/download allow, with
direct connections, on the same network, across town and to Korea. This night: measure
the raw links, measure DropBeam on the same pairs, find the gaps, fix what is DropBeam's.

Branch `netlab`. All DropBeam numbers come from `dropbeam-lab` (the real engine,
production preset, lab-only identities; see LAB.md), receiver verified by sha256.

## Machines and links

| Machine | Network | Link |
| --- | --- | --- |
| MacBook Air M4 ("Mac") | home, 192.168.86.x | Wi-Fi 5 GHz ch 157/80 MHz, 780 Mb/s PHY, −57 dBm |
| ass-box (N95, Ubuntu 24.04) | home, 192.168.86.42 | Wi-Fi RTL8821CE (`rtw88_8821ce`), ch 149/155, −43 dBm, 2 unused gigabit ports |
| linux-agent (N95) | office, 192.168.1.108 | gigabit Ethernet (+ Wi-Fi .177) |
| windows-agent (OptiPlex, Win 10) | office, 192.168.1.149 | gigabit Ethernet |
| Linux M (MacBook Air M5, macOS) | Korea, 116.46.x | Wi-Fi 6 ch 36/80 MHz, 720 Mb/s PHY |

## Raw baselines (iperf3, 10 s; Mbit/s; "TS" = over the Tailscale IP)

| Pair | RTT ms (avg) | TCP 1 / 4 streams → | TCP 1 / 4 streams ← | UDP capacity → / ← |
| --- | --- | --- | --- | --- |
| Mac → ass-box (LAN) | 23 (8–100, jitter) | 44.7 / 54.6 | 4.1 / 4.0 | 145 (24 % loss) / 1.4 |
| Mac → ass-box (TS) | 13 | 61.1 / 44.9 | 4.0 / 4.0 | 103 / 0.7 |
| windows-agent → linux-agent (office LAN) | <1 | 492 / 504 | 809 / 891 | n/a (iperf 3.1.3 UDP hangs on Windows) |
| Mac → linux-agent (town) | 46 (25–97) | 12.9 / 17.5 | 39.4 / 53.0 (8 streams 66) | 20.8 / 210 (at 250 offered, 16 % loss) |
| windows-agent → Mac (town) | 38 | 32.5 / 42.9 | 11.0 / 15.0 | — |
| ass-box → linux-agent (town) | — | 4.2 / 4.0 | 73.5 / 27.5 | 2.6 / 59 (at 60 offered) |
| Linux M → Mac (Korea) | 256 | 0.8 / 1.0 | 6.0 / 8.9 | 7.8 / 20 (at 40 offered, 49 % loss) |
| Linux M → linux-agent (Korea) | 190 | 1.5 / 2.3 | 0.8 / 45.8 | 8.0 / ≥ 93 (sender-limited, 0.1 % loss) |
| Linux M → ass-box (Korea) | 234 | 0.6 / 0.8 | 0.5 / 3.3 | 7.8 / 0.8 |

Bottlenecks that follow: home upload ≈ 18–21 Mb/s; home download ≥ 170–210; office
upload ≥ 93; office LAN ≈ 0.5–0.9 Gb/s; **ass-box upload ≈ 4 Mb/s**; **Korea upload
≈ 8 Mb/s with ~8 % loss even at 6 Mb/s offered** (non-congestive — a lossy hop), Korea
download ≥ 90.

## DropBeam vs raw (auto mode, default parallel; Mbit/s)

"% of bottleneck" = DropBeam / best raw rate for that direction (UDP capacity where
known, else best TCP).

| Pair | Path chosen | DropBeam (runs) | Raw bottleneck | % | vs best TCP |
| --- | --- | --- | --- | --- | --- |
| Mac → ass-box (home LAN) | local 192.168.86.42 | 76.5, 69.7, 82.0; 1 stream 75.0 | ~100 (UDP 145 @ 24 % loss) | ~75 | 1.4× |
| ass-box → Mac (home LAN) | local | 3.6, 3.8 | 4.1 | 90 | ≈ |
| windows → linux-agent (office LAN) | "local" (really via Tailscale, below) | 445, 412, 450, 349, 282, 387, 276 | 504 | 55–89 | ≈ |
| linux-agent → windows (office LAN) | Tailscale 100.77.x (lab exe has no firewall rule) | 558, 328 | 891 | 37–63 | — |
| Mac → linux-agent (town) | linux-agent's office Wi-Fi IP via Tailscale subnet route | 17.9; 1 stream 15.3 | 20.8 | 86 | 1.0× |
| linux-agent → Mac (town) | public 72.128.x | 159, 177 | 210 | 76–84 | 2.7× |
| Mac → windows (town) | direct | 16.4, 14.0 | ~18–21 | ~80 | 1.0× |
| windows → Mac (town) | direct | 120, 170 | 210 | 57–81 | 2.8–4× |
| ass-box → linux-agent | direct | 3.8, 3.9 | 4.2 | 92 | ≈ |
| linux-agent → ass-box | direct | 79, 94 | ≥ 74 (TCP) | ≥ 100 | 1.1–1.3× |
| linux-agent → Linux M (Korea) | public 116.46.x or Tailscale | 78.4, 77.9, 78.8; 68–69 (rel); 1 stream 69 | ≥ 93 | 73–85 | 1.7× |
| Linux M → linux-agent (Korea) | Tailscale | 4.2–5.6 (≈ 4.6 typical) | 8.0 (8 % loss) | 55–70 | 2–3× |
| Mac → Linux M | direct | 16.7, 15.5 | ~18–21 (home up) | ~80 | 1.8× |
| Linux M → Mac | direct | 2.5, 2.4 | 7.8 | 31 | 2.5× |
| ass-box → Linux M | direct | 3.3, 2.8 | 4.2 (ass-box up) | 70 | ≈ 1× (P4 3.3) |
| Linux M → ass-box | relay → direct | 4.2, 4.3 | 7.8 | 55 | 5× |
| relay only, Mac → linux-agent | n0 use1 | 6.8 | — | — | — |
| relay only, linux-agent → Mac / windows → linux-agent | n0 use1 | 8.6 / 8.7 | — | — | — |
| relay only, linux-agent → Linux M | n0 aps1 | 1.5 (RTT 996 ms) | — | — | — |
| relay only, Mac → ass-box (same LAN) | n0 use1 | 8.2 | — | — | — |

Small-file timings (quick suite, all PASS): connection setup ≈ 1 RTT (2–3 ms office LAN,
15–50 ms home/town, 180–300 ms Korea); first confirmed byte ≈ 1–2 RTT + the integrity
handshake (14–400 ms LAN/town, 0.8–1.4 s Korea).

DropBeam already matches or beats TCP everywhere (BBR + QUIC vs the OS's CUBIC), and is
at 75–100 % of the bottleneck on every clean path. The two real gaps are the lossy Korea
uplink (55–70 %, and only 31 % Korea → home) and the public relays.

## Findings

1. **The slow home LAN copy (0.27 MB/s SSH, 0.45 MB/s DropBeam) is ass-box's Wi-Fi
   transmit, not DropBeam, the Mac or Tailscale.** ass-box uploads at ~4 Mb/s to every
   destination (LAN IP or Tailscale, Mac or office) while it downloads at 45–100 Mb/s.
   SSH Mac → ass-box now runs 4.3 MB/s; ass-box → Mac 0.41 MB/s. The `rtw88_8821ce`
   driver's rate control is stuck: debugfs `phy_info` reports `[Tx Rate] = CCK_1M` (not
   even a 5 GHz rate) while it receives at VHT MCS8/9. Power save off: no change.
   Forcing the TX rate for 25 s (`echo 0x31 > /sys/kernel/debug/ieee80211/phy0/rtw88/fix_rate`,
   VHT 1SS MCS5, auto-reverted to 255) took ass-box → Mac TCP from 4 to **58 Mb/s**.
   Owner fix (not applied — it's the box's OS config): plug ass-box into Ethernet (it has
   two unused gigabit ports), or a newer rtw88 driver/kernel, or a boot-time `fix_rate`.
   DropBeam itself gets 90 % of those 4 Mb/s.
2. **Mac Wi-Fi latency jitter**: Mac → router ping 2.7–73 ms (ass-box → router 6–15 ms);
   `awdl0` (AirDrop/AWDL channel hopping) is active. It costs throughput on the Mac's
   LAN paths; DropBeam's min-RTT path logic already ignores it.
3. **Office "LAN" traffic from windows-agent goes through Tailscale.** linux-agent
   advertises 192.168.1.0/24 as a Tailscale subnet route and windows-agent accepts it:
   `route print` shows 192.168.1.0/24 via 100.100.100.100 at metric 5 beating the on-link
   route (281); `ping -f -l 1400 192.168.1.108` fails ("needs to be fragmented") and the
   first hop is 100.77.104.58. Every byte between the two office machines is
   WireGuard-encrypted on the old OptiPlex, so DropBeam (and iperf3) cap at ~0.3–0.5 Gb/s
   on gigabit Ethernet and QUIC's MTU is 1200–1230 instead of 1452. DropBeam calls it
   "local" (the address is on-subnet). Owner fix: on office PCs `tailscale set
   --accept-routes=false`, or stop advertising the office subnet from linux-agent (only
   needed for off-site access). Same for the Mac across town: it reaches linux-agent's
   office Wi-Fi address 192.168.1.177 through the subnet route.
4. **Korea uplink: ~8 % random loss.** UDP at 6 Mb/s offered loses 8.2 %, so it is not a
   policer or a queue; RTT doesn't grow under load. TCP collapses (0.6–2.3 Mb/s);
   DropBeam gets 4–5.6 Mb/s. Rate-over-time: a slow ramp (BBRv3 exits startup on the
   loss) to ~6 Mb/s, then ~4–5. Korea → home is worse (2.4 Mb/s) than Korea → office
   (4.6) — the home download path adds loss/jitter.
5. **Path family**: when both a public and a Tailscale path exist DropBeam picks either
   (equal RTT). Interleaved A/B linux-agent → Linux M, 64 MiB × 3: public 67.4 / 70.9 /
   70.2 vs Tailscale 73.0 / 74.5 / 70.8 Mb/s. Tailscale costs ~19 % more packets (MTU
   1230 vs 1452) but no throughput here, so the selector was left alone.
6. **Relays**: n0's public relays deliver ~7–9 Mb/s inside the US and 1.5 Mb/s to Korea
   (RTT inflates to ~1 s: QUIC rides one TCP/WebSocket whose transpacific leg behaves
   like single-stream TCP, 0.8 Mb/s in iperf3). The relay is only for the first
   packets/hard NATs; every pair here went direct within the first round trips except
   one small case (Linux M → ass-box started on the relay, upgraded mid-transfer).
   A DropBeam relay near Korea (RELAY-SETUP.md, "Custom relay") is the fix for
   relay-only Korea peers.
7. **Failover on a dead direct path takes 15–20 s.** Blocking the Mac's LAN UDP on
   ass-box mid-transfer (relay still reachable): a 10 s outage stalled ~12 s and resumed
   on LAN; a 35 s outage stalled ~20 s, then continued over Tailscale (whose own direct
   path was also cut, so via DERP at ~12–17 Mb/s), never on the n0 relay. Both
   transfers completed and verified. The 20 s = iroh's `PATH_MAX_IDLE_TIMEOUT` (15 s)
   plus re-selection. With `DROPBEAM_PATH_IDLE_MS=5000` on both ends the 35 s outage
   stalled 6 s instead of 20 s. Not shipped (see "What's left").
8. **Windows lab binary was broken**: `dropbeam-lab.exe` exited before `main` with
   0xC0000139 on Windows 10 — tao imports `TaskDialogIndirect` from comctl32 v6 and the
   console exe had no Common-Controls 6 manifest. Fixed (labtool/build.rs). The app was
   never affected (tauri-build embeds the manifest).

## Experiments that did not ship (A/B, interleaved; Mbit/s)

| Knob | Pair | Base | Variant | Verdict |
| --- | --- | --- | --- | --- |
| send window 16 / 32 MiB (8 MiB shipped) | linux-agent → Korea, 96 MiB × 2 | 77.9, 78.8 | 16 MiB: 74.3, 81.0; 32 MiB: 80.2, 75.6 | no gain (RTT grows 190 → 400 ms: the path is full) |
| congestion control: stock BBRv3 / CUBIC | Korea uplink, 16 MiB | 4.6 | 3.9 / 4.1 | patched BBRv3 stays |
| ACK frequency off (threshold 10 shipped) | Korea uplink | 4.6 | 3.7 | keep |
| initial window 32 KiB (128 KiB shipped) | Korea uplink | 4.3–4.6 | 5.6, 4.8 | noisy, not enough to undo the photo-latency tuning |
| BBRv3 "random-loss tolerance" (skip the 0.7 lower-bound cut for loss rounds with no queue; patch in vendored noq-proto) | Korea uplink, 6 pairs | 4.3, 4.5, 4.2, 4.7, 4.5 | 4.8, 4.7, 4.5, 4.5, 4.2 | within noise, 2× the loss — reverted |
| 4 streams on LAN (1 shipped) | windows → linux-agent, 512 MiB × 2 | 450, 349 | 424, 371 | keep 1 |
| parallel off (1 stream) on internet | town / Korea | 17.9 / 78.4 / 4.7 | 15.3 / 69.0 / 4.2 | keep 4 (one exception: linux-agent → Mac 159 vs 177) |

## Reliability

| Pair | Small (quick suite × 10 = 40 transfers) | Big |
| --- | --- | --- |
| linux-agent → Linux M | 40 / 40 | 3 / 3 (64 MiB) |
| Linux M → linux-agent | 37 / 40 — one run: the connection timed out twice in a row (QUIC idle timeout = no packets for 30 s, a Korea-side outage) and one result never reached the verifier | 2 / 2 (16 MiB) |
| windows → linux-agent | 40 / 40 | 3 / 3 (512 MiB) |
| every other pair (quick + 2 big) | all PASS | all PASS |
| Mac → ass-box with 10 s / 35 s LAN outage | — | 2 / 2 PASS (stall 12 s / 20 s) |

Resume across a reconnect and chat delivery were not exercised live: `dropbeam-lab serve`
writes every connection to a fresh folder (so it can't resume), and the `dropbeam` CLI
drives a running app whose config folder can't be pointed at a lab-only directory. Both
are covered by the engine's unit tests (resume/coverage sidecar, chat), not by this lab.

## Changes on `netlab`

- `iroh_net::transport_config()` — one QUIC transport definition shared by the app and
  every lab endpoint. The lab carried its own copy and had drifted (no multipath-path /
  NAT-address headroom), so lab numbers did not describe the shipping app. No change to
  the app's values.
- Lab-only env overrides (unset in the app): `DROPBEAM_STREAM_WINDOW`,
  `DROPBEAM_SEND_WINDOW`, `DROPBEAM_LAN_STREAMS`, `DROPBEAM_NET_STREAMS`,
  `DROPBEAM_PATH_IDLE_MS` (existing: `DROPBEAM_CC`, `DROPBEAM_INITIAL_WINDOW`,
  `DROPBEAM_ACK_THRESH`).
- `dropbeam-lab send`: `connectMs` and `firstByteMs`; the case clock now starts after
  hashing the expected files (hashing used to be counted — ~1 s per 128 MiB);
  `--mode public|overlay`; retry of the round-boundary reset.
- `dropbeam-lab serve`: waits for the puller to close the results connection (dropping it
  raced the read, "connection lost", and aborted whole runs).
- `labtool/build.rs`: Common-Controls 6 manifest for the Windows exe.
- `.github/workflows/lab-build.yml`: push to `lab-build` → `dropbeam-lab.exe` artifact.

No engine tuning change shipped: every knob tried was within noise or worse on the
measured pairs, and the engine is already at 75–100 % of the bottleneck where the path
is clean.

## What's left

- **Path failover** (finding 7): a 5–7 s direct-path idle timeout cuts a 20 s stall to
  ~6 s. Before shipping, retune keep-alives together with it (iroh heartbeats idle paths
  every 5 s; a 5 s timeout would abandon healthy idle paths after one lost PING and churn
  paths — the 2026-09-12 path-id exhaustion), and test iOS (battery) and Wi-Fi roaming.
  Also: during the outage the healthy n0 relay path carried almost nothing — prefer the
  relay over a still-silent direct path sooner.
- **Lossy uplinks** (Korea): BBRv3's loss response limits a 8 %-random-loss uplink to
  ~60 %. The tried random-loss patch didn't help; next candidates: lower-bound cut only
  when the round's loss exceeds 2 % AND the RTT rose, or FEC.
- **Relay to Korea**: run a DropBeam relay in Asia (or on linux-agent with a forwarded
  port) for relay-only Korea peers.
- **Owner network fixes** (not DropBeam): ass-box on Ethernet / rtw88 rate fix; office
  Tailscale subnet route vs `accept-routes` on office PCs; the Mac's AWDL jitter.
- Linux receivers clamp the 7 MB socket buffer to `net.core.rmem_max` (212 992 here);
  irrelevant at these rates, worth `sysctl net.core.rmem_max=7340032` on a gigabit
  Transfer Server box.
- Lab: an `--resume` mode (fixed destination) and a lab-only config dir for the CLI so
  resume and chat can be tested live.
