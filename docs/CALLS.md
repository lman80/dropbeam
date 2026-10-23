# Calls — audio and video between friends and your own devices

Owner's decision (2026-10-23, "Option B"): calls stay fully decentralized. No call
servers, no STUN/TURN on the internet, no paid or third-party media service. Calls
use the same iroh connection as files and chat, and recording happens only on the
device, only with the other person's yes.

Code: `src-tauri/src/calls/` (engine), `src/lib/calls.ts` + `src/components/CallScreen.tsx`
(desktop call screen), `src-tauri/plugins/native-ui/ios/Sources/NativeUIPlugin/Calls/`
(iOS), `push-worker/worker.js` route `/call` (iPhone wake-up only).

## How a call is built

```
 call screen (WebRTC)                     call screen (WebRTC)
   RTCPeerConnection                        RTCPeerConnection
   iceServers = [turn:127.0.0.1:P]          iceServers = [turn:127.0.0.1:Q]
   iceTransportPolicy = "relay"             iceTransportPolicy = "relay"
        │ UDP (loopback only)                      │
   ┌────▼─────────────┐   QUIC DATAGRAMs    ┌──────▼───────────┐
   │ TURN bridge      │◄═══════════════════►│ TURN bridge      │
   │ (turn.rs/media.rs)│  on the friend's   │                  │
   └──────────────────┘  iroh connection    └──────────────────┘
        signaling: one "call.offer" stream per device, open for the whole call
```

- **Media engine = WebRTC.** Echo cancellation, noise suppression, gain control, Opus,
  jitter buffer, FEC, congestion control (GCC), H.264/VP8. Desktop: the web view's own
  WebRTC (WKWebView on macOS, WebView2 on Windows). iOS: Google's WebRTC framework.
  Nothing hand-made in the media path.
- **Path = iroh.** The WebRTC engine may use exactly one ICE server: a TURN server
  DropBeam runs on `127.0.0.1` for this call only (`calls/turn.rs`, the subset of
  RFC 8656 a WebRTC TURN client needs: Allocate with the 401/nonce long-term
  credential dance, Refresh, CreatePermission, ChannelBind, Send/Data indications,
  ChannelData). Its "relayed addresses" are virtual (`10.77.0.1` = caller,
  `10.77.0.2` = callee). Every packet WebRTC sends to the other side's virtual address
  is put in one unreliable QUIC DATAGRAM on the friend's iroh connection
  (`calls/media.rs`) — no retransmission, no head-of-line blocking, so WebRTC sees a
  plain lossy network. iroh picks the path as it always does: same network → direct
  internet (hole-punched) → relay.
- **Packets larger than a datagram** (the limit is the path MTU minus QUIC overhead,
  ~1.15–1.4 KB) are split into up to 8 pieces and rejoined; losing a piece loses the
  packet, like IP fragmentation. Measured: few or none (WebRTC keeps RTP packets
  ~1.2 KB).
- **Credentials**: random username/password per call, handed only to this app's call
  screen; the socket binds `127.0.0.1` only. A wrong password gets 401; allocations
  are capped at 16 (one per network interface of the WebRTC engine).

### Signaling (`calls/mod.rs`, `calls/signal.rs`)

The caller opens one bidirectional stream per device of the person called (their
Mac, their iPhone…) and sends `call.offer {callId, tag, video, sdp}`. The stream
stays open as the call's line. Frames:

| Frame | Meaning |
|---|---|
| `call.ring` | this device is ringing |
| `call.answer {sdp}` | answered here (first one wins) |
| `call.busy` / `call.decline` / `call.missed` | busy, said no, rang out (45 s) |
| `call.unavailable {reason?}` | not reachable for calls (`unsupported`: this device can't take calls) |
| `call.cancel` | caller stopped before an answer |
| `call.taken` | another of your devices answered — stop ringing |
| `call.glare` | we are calling you at this moment and our call wins (honoured only on your own ringing call to that device, and only if the rule below agrees) |
| `call.hangup` | end |
| `call.media {muted, camera}` · `call.record {action}` · `call.sdp` | mid-call |

- **One call at a time.** A ring while busy is answered `call.busy` and logged as a
  missed call.
- **Several devices ring; the first to answer wins** — the others get `call.taken`
  (no history line on them). A decline on any device ends the call for all.
- **Glare** (both call each other at once): both sides compare the two devices'
  endpoint ids (never the call ids — a caller picks those); the call placed by the
  smaller one survives and is answered automatically on the other side. That is the
  only call ever answered without a tap, and only while my own call to that very
  device is still ringing: a `call.glare` on a call *they* placed, after an answer, or
  claiming a win the rule denies is ignored, and nothing is remembered for a later call
  (my call waits at most 10 s for theirs). The auto-answered call is never more than
  either side was calling with: video only if both called with video (`video =
  theirs && mine`), so my audio call never turns my camera on.
- **Timeouts**: callee rings 45 s; caller gives up at 50 s; answered calls must get
  media flowing within 30 s.
- **Non-trickle ICE**: each side waits for gathering to complete (fast — the only
  candidates are the local TURN allocations) and sends one description.
- The ending side's device writes a **call line** into the chat thread (`kind:"call"`,
  `call: {video, outcome, durationMs}`), id `call-<callId>` so own devices that sync
  chats merge it into one line. Each device writes its own; nothing is sent.

### Quality settings

- Opus 48 kHz mono, in-band FEC on, DTX off, up to 128 kbps (`calls/sdp.rs` rewrites
  each side's own Opus `fmtp`; `maxBitrate` 128 kbps and high network priority on the
  audio sender), 20 ms packets.
- Video 1280×720 at 30 fps, up to 2.5 Mbps; WebRTC scales resolution/bitrate down
  with the measured bandwidth. H.264 first on Apple devices (hardware encoder),
  otherwise the engine's order (VP8/VP9/H.264).
- Audio and video transceivers always exist, so the camera can come on in an audio
  call without renegotiating.

## Security model

- **Who can ring you**: only friends and your own devices. Identity is the iroh
  connection's authenticated `remote_id()`; a name in a frame is never trusted.
  Blocked people get exactly what a stranger gets (`call.unavailable`).
- **Anti-spam ringing**: per calling device, at most 5 rings a minute and 30 an hour;
  more are answered `call.unavailable` without ringing.
- **End-to-end encryption**: WebRTC's DTLS-SRTP, on top of iroh's QUIC/TLS. The DTLS
  fingerprint travels only inside the authenticated iroh stream; `calls/sdp.rs`
  refuses any description without a `sha-256`/`384`/`512` fingerprint (or with a
  `sha-1` one). Neither the bridge nor iroh's relay can swap the media keys: WebRTC
  checks the peer's DTLS certificate against the signaled fingerprint.
- **The other side can't steer your WebRTC engine**: incoming descriptions keep only
  `typ relay` UDP candidates on `10.77.0.x`; the TURN server also refuses permissions
  and channels for any peer other than the other side's virtual address (403).
- **Connections carrying a call are never rotated** by the friend-connection
  housekeeping (`iroh_net::cached_friend_conn`).
- Agents (`agent_ops.rs`): `calls.status`, `calls.history`, `calls.recordings` (read),
  `calls.start` and `calls.hangup`. `calls.start` only *asks*: the call screen opens
  with "Call <name>? (audio/video) — Call / Cancel" (`call://dial` carries
  `confirm: true`); nothing rings and no microphone or camera opens until a person
  clicks Call. Answering, recording and the WebRTC plumbing are person-only (EXCLUDED
  with reasons).

## Recording (on the device, with consent)

- **Record** on the call screen sends `call.record {action:"ask"}`. The other side sees
  "<Name> wants to record this call on their device — Allow / Don't allow".
- `call_record_open` is refused until an `allow` for this call arrived. The **engine**
  (not the screen) sends `started`/`stopped`, so the other side's red "Recording"
  badge can't be skipped by a modified call screen on the recorder's side.
- **Taking it back**: a `deny` while recording closes the recording at once (the engine
  sends `stopped`, both badges go off) and the engine refuses every later chunk; what
  was recorded while they allowed it stays.
- **What this does and doesn't protect**: consent and the badge bind *honest* DropBeam
  apps only. A modified app (or any screen recorder) on the other side can record what
  it plays and captures without asking, as with any call app. They stop an unmodified
  DropBeam from recording you without your yes, and tell you when it does.
- Desktop: MediaRecorder writes three files to
  `<Downloads>/DropBeam Calls/<date time> <Name>/` under fixed names, whatever the
  friend is called: `Me.webm` (your voice), `Them - <Name>.webm` (their voice),
  `Call.webm` (both voices mixed, with their video when there was any) and `call.json`
  (participants, start/end, which file is whose). Nothing is uploaded anywhere.
- **Swap clean tracks** (for AI notes): while recording, either side can offer to swap;
  when both agreed (`call.record {action:"share"}` both ways), after the call each side
  sends its own `Me` track to the other through the normal verified file transfer, so
  each ends up with both clean voices.
- iOS: same consent and badge. DropBeam drives the iPhone's echo-cancelling
  voice-processing audio unit itself (a custom WebRTC audio device on the same unit
  WebRTC's built-in device uses). A recording gets your voice after echo cancellation
  (silent while muted) and exactly what plays from the other side. Each voice and the
  mixed call are encoded on the phone as AAC (`Me.m4a`, `Them - <Name>.m4a`, `Call.m4a`). When
  recording stops or the call ends, they move into Documents/DropBeam Calls/<date> <name>/
  next to `call.json`, which shows in the Files app under DropBeam. The engine accepts
  the files for 10 minutes after the call closed the recording, and only from the app's
  own temp folder. If both of you agreed to swap tracks, after the call the iPhone sends
  your `Me.m4a` as a normal file in the chat. Not yet tested on a device.

## Platforms

| | macOS | Windows | Linux | iOS |
|---|---|---|---|---|
| Audio/video calls | ✅ tested (WKWebView) | ⚠️ builds in CI; WebView2 has WebRTC, not call-tested | ❌ Ubuntu's WebKitGTK ships without WebRTC (checked 2.52 on 24.04: `RTCPeerConnection` is undefined) → "can't take calls yet" | ⚠️ builds + launches in the simulator; not call-tested |
| Ringing while hidden/in tray | ✅ window comes up + notification | ✅ same code | — | CallKit; VoIP push when backgrounded (owner setup) |
| Recording | ✅ (desktop code; mic recording needs a real mic) | ⚠️ | — | ⚠️ built, compiles, not run on a device |

Linux shows the call buttons and explains why a call can't start; friends calling a
Linux device read "<Name>'s device can't take calls yet".

### Plan: native calls on Linux (not built yet)

Evaluated 2026-10-23 and deferred — not doable in one night without risking the Linux
build for everyone:

- **Engine**: LiveKit's `libwebrtc` crate (safe Rust over `webrtc-sys`, Apache-2.0;
  prebuilt Google libwebrtc downloaded by its build script). Since webrtc-sys 0.3.x it
  compiles against the libc++ bundled in that artifact and **needs clang ≥ 21**
  (Ubuntu 24.04 ships 18 → apt.llvm.org on every build machine and CI runner), plus
  `libglib2.0-dev libclang-dev libjpeg-turbo8-dev`. The download is ~100–200 MB per
  target.
- **Shape**: a cargo feature `native-calls` (Linux only, off by default, so CI and the
  other OSes are untouched until it's proven). A `calls/native.rs` drives
  `PeerConnectionFactory` with the **same** configuration as the call screens
  (ICE server = the per-call 127.0.0.1 TURN bridge, relay policy, Opus fmtp from
  `sdp.rs`) — signaling, consent, recording files and history stay exactly as they are.
- **Audio**: libwebrtc's own Linux audio device module (PulseAudio/PipeWire-pulse,
  with its echo canceller), or `NativeAudioSource`/`NativeAudioStream` fed from/to
  `cpal`. Audio first; video via `NativeVideoSource` from a V4L2 camera (`nokhwa`) and
  remote frames drawn into the WebKitGTK page as JPEG/RGBA frames over the local range
  server (`stream.rs`) — slower than a native surface, fine for 720p.
- **UI**: the existing call screen with a thin "engine" switch in `src/lib/calls.ts`:
  when `RTCPeerConnection` is missing but `call_status().native` is true, the screen
  sends button presses to Rust commands instead of driving WebRTC itself.
- **Steps**: (1) build `libwebrtc` on ass-box with clang-21 behind the feature; (2) an
  audio-only call Linux ↔ Mac through the bridge (the lab call node is the harness);
  (3) CI job with clang-21 for the feature; (4) video; (5) turn the feature on for the
  Linux release.

### iOS

The iPhone uses Google's WebRTC (stasel/WebRTC M155, BSD-3-Clause), fetched and
checksum-checked at build time by `plugins/native-ui/ios/fetch-webrtc.sh` and embedded
in the app, configured exactly like the desktop call screen: its only ICE server is
DropBeam's TURN bridge on 127.0.0.1 (relay policy), audio and video channels always
present, Opus up to 128 kbps with FEC, H.264 (hardware) preferred, 720p30 up to
2.5 Mbps. CallKit shows incoming calls (also on the lock screen) and owns the audio
session; WebRTC's audio only runs between CallKit's activate and deactivate. Mute, hold
(treated as mute) and end in the system UI map to DropBeam's actions. The SwiftUI call
screen has mute, camera, flip, speaker, end, the quality pill and the "Recording"
badge, and answers the other side's recording request. Calls start from the chat's
"…" menu; call lines in the thread call back. Without CallKit (simulator) it rings
in-app.

### Waking an iPhone for a call (VoIP push)

In each hello to a friend or own device, the iPhone includes its PushKit VoIP token
sealed to the push relay's key and bound to that one device — only that device can use
it, nobody but the relay can read it. When dialing an iPhone fails, the caller signs a
`/call` request with its device key; the relay checks the signature, that the token
was sealed for this caller, and its limits (one push per phone per call per minute,
per-caller and per-phone limits per minute and hour, a global cap), then asks APNs for
a VoIP push (priority 10, 30 s expiry). The caller's identity is sealed to the phone's
notification key, so neither the relay nor Apple learns who is calling; the payload is
only `{c: call id, v: video, s: sealed}`. The phone reports the call to CallKit at once
(iOS requires it); a caller who isn't a friend is ended immediately, otherwise the
engine starts and the caller — redialing every 2 s while it rings — reaches it. Only
the wake-up uses the relay; the call itself always runs over iroh. Without a
configured relay or VoIP token, calls ring only while DropBeam runs.

Who the lock screen names, and what a lock-screen answer connects to:

- The sealed part is `{f: caller, c: call id, sig}`, `sig` = the caller's Ed25519
  signature (its endpoint key) over `"dropbeam-call-wake-v1\n" + c + "\n" + f`. The
  phone's push key is public to every friend, so without the signature one friend
  could seal another friend's id. The phone shows a name only when the signature
  checks, `c` is this push's call id and `f` is in its contacts; otherwise the CallKit
  call ends at once. A plain `f` (no push key known yet) must equal the request's
  verified signer — the relay refuses anything else.
- A call answered on the lock screen before the engine had it connects only if the
  engine's call comes from that same device `f` (the engine has checked it is a friend
  or own device). A different caller ends that ring and rings normally.
- Limits: per-minute limits and the one-push-per-call dedupe use Cloudflare
  rate-limit bindings (`RL_SERVER`, `RL_TOKEN`, `RL_DEDUPE`, `RL_GLOBAL`; per
  Cloudflare location). The hourly budgets are per isolate (best effort): the relay
  deliberately has no KV or Durable Object (a KV write quota anyone can exhaust would
  be a kill switch).
- Revocation: the sealed token expires after 30 days and each hello renews it; a
  removed or blocked friend gets no new hellos, and until their copy expires a push
  from them only wakes the phone for a CallKit report that ends at once (they are no
  longer in its contacts). The relay keeps no per-caller revocation list.

**Owner-only steps**: (1) docs/PUSH-SETUP.md steps 1–4 if not done (the APNs auth key
also covers VoIP pushes; no separate VoIP certificate); (2) `cd push-worker && wrangler
deploy` (no new secrets); (3) the "Voice over IP" background mode is in the app's
Info.plist — App Review may ask about CallKit/VoIP use; (4) test on a real iPhone:
answer from the lock screen, app killed when the call arrives, two-way audio with
CallKit active.

## Tests and measurements

Unit/integration (`cargo test --lib calls::`): STUN codec against RFC 5769 vectors and
MD5 vectors; TURN allocate/401/438/permission/channel/refresh/expiry/quota and peer
restrictions; datagram framing, fragmentation, reassembly limits; SDP checks and Opus
tuning; consent rules; the call state machine (first-answer-wins, busy, decline,
missed, no answer, cancel, connect timeout, glare); and end-to-end over loopback iroh
with real endpoints and the real accept loop: a friend call that rings, answers,
carries media through both TURN bridges as datagrams (including a fragmented 3 KB
packet), passes mid-call frames, hangs up and logs both sides; strangers and blocked
people can't ring; a device without WebRTC says so; busy/decline/missed; rate limit;
two devices of one person (first answer wins, the other stops); glare (settled by
endpoint id, no audio→video upgrade); a forged `call.glare` on the caller's own call
answers nothing; `call.glare` is ignored unless my ringing call to that device loses;
taking recording consent back stops the recording and refuses later chunks; fixed
recording file names; an agent's dial only asks.

`calls::lab_tests` (`#[ignore]`, needs Chrome): real WebRTC in headless Chrome with fake
camera/mic, two peer connections each limited to its own bridge, bridges joined by a
loopback iroh connection.

Live (debug builds; `dropbeam-lab call-node` runs the engine headless with a Chrome
call screen; `DROPBEAM_CALL_FAKE_MEDIA=1` gives the desktop app a synthetic
camera/mic).

### Audio delay, measured with speech (round 2)

The first round's audio numbers came from a synthetic Web Audio beep track. That source
is bursty on its own: on a 1 ms loopback with no loss, the receiver's jitter buffer still
grew to 350 ms. Those numbers are withdrawn. The harness now feeds a 48 kHz speech WAV
(`say` → `afconvert`) through Chrome's real capture path
(`--use-file-for-fake-audio-capture`, with echo cancellation, noise suppression and
gain control on) and plays the far side through an `<audio>` element like the call
screen. `calls::lab_tests::webrtc_speech_latency_in_chrome` (`LAB_WAV=…`) puts the
captured track and the received track into one AudioContext and cross-correlates their
energy envelopes. That gives capture → receiver output, i.e. mouth-to-ear minus the
sound card's own buffers, which typically add 10–20 ms on each side.

| Speech, same machine (ass-box, 2× Chrome over loopback iroh) | mouth→ear | jitter buffer |
|---|---|---|
| Chrome's own direct path (baseline, no DropBeam) | 43 ms (2 runs) | 30–38 ms |
| Through the TURN bridges and iroh datagrams | 43–85 ms (8 runs, median ≈ 55) | 30–40 ms |

Concealment appeared only in runs where the box was overloaded (load 5 on 4 cores).
**LAN target (< 150 ms) is met.**

**Bug found and fixed: the ACK frequency starved low-rate flows.** Across town, arrival
gaps of 120–200 ms showed up at the receiving bridge (25 in 40 s) while plain ping stayed
under 63 ms. DropBeam asks peers to acknowledge only every 10th packet (good for bulk
transfers). A call sends 50–150 packets/s, so it was acknowledged about once per round
trip, and the congestion window stalled the sender in bursts. The far side's jitter
buffer grew to 170–240 ms. With ACK frequency off it was 54–83 ms. Fix
(`iroh_net::ack_frequency`): the requested max ACK delay is capped at 5 ms
(`DROPBEAM_ACK_DELAY_MS` overrides). A bulk transfer still reaches the 10-packet
threshold first, so it keeps the reduced ACK rate. After the fix: no gaps > 120 ms, and
jitter buffer 62–87 ms (one direction) and 63–133 ms (other, over the home uplink).

**Queue bound**: when the congestion window is full, QUIC would queue up to 1 MB of
datagrams. That is seconds of video, and audio would wait behind it. The bridge now
drops large (video) packets once ~80 ms of the current send rate is waiting (at least
8 KB). Small (audio/RTCP) packets always go. WebRTC sees ordinary loss and lowers its
bitrate.

| Speech run (real capture path, after the fixes) | Path | Media RTT (= iroh RTT ≈ ping) | Jitter buffer | One-way audio ≈ RTT/2 + buffer + ~20 ms | Loss | Video |
|---|---|---|---|---|---|---|
| linux-agent ↔ ass-box, across town | direct | 36–40 ms | 62–133 ms | ~100–170 ms | ≤ 0.4 % | 720p |
| ass-box → Korea (minkyung-mac; speech received in Korea) | direct (hole-punched) | 232–252 ms (ping 216–370, mdev 31) | 223 ms (269 before the queue bound) | ~360 ms | ≤ 5.6 % | 720p |

**Korea path**: media RTT equals iroh's RTT and plain ping (234 vs 238 ms), and the path
never fell back to relay (`direct` in every sample). Datagram size is fine: max datagram
1192–1414 bytes, 0–14 packets fragmented in a minute, none dropped for size. The loss
and jitter are the path's own: ping alone shows max 370 ms and 31 ms deviation, and the
Korea Mac is on Wi-Fi. WebRTC's jitter buffer follows that jitter. The queue bound cut
gaps > 120 ms from 19 to 1–5 per minute and the buffer from ~250–270 to ~220 ms.
Further gains would need a calmer network. The "< 100 ms + one way" target is met
across town (~19 ms one way + ≤ 100 ms in the best direction), not on this
trans-Pacific Wi-Fi path.

### Earlier runs (round 1, synthetic media — video numbers still valid)

| Run | Path | Setup | Media RTT | Video (glass→glass) | Loss | Video |
|---|---|---|---|---|---|---|
| ass-box, loopback, 2× Chrome | local | 1.0–1.3 s | 3–19 ms | — | 0 % | 720p |
| This Mac, 2 app instances (WKWebView) | local | 0.5 s | 1–3 ms | 40–55 ms | 0 % | 720p30 H.264 |
| Mac (Wi-Fi) ↔ ass-box, home LAN | local | — | 22–45 ms | ~250 ms ¹ | 0 % | 720p |
| linux-agent ↔ ass-box, relay forced | relay | 1.1 s | ~81 ms | ~150 ms | 0 % | 720p |
| Mac (Wi-Fi) ↔ Korea | direct | — | 240–345 ms | ~610 ms ¹ | ≤ 7 % | 720p / 360p |

¹ CPU-bound headless 720p software encoding on ass-box and Wi-Fi jitter. These ran
before the ACK-delay and queue fixes.

**Not verified**: a real camera and microphone (macOS permission prompts need a person),
Windows calls, an iPhone (real device, CallKit, VoIP push), recording with a real
microphone (consent flow is tested in code; the MediaRecorder path in a real call is
not), echo cancellation quality, 1080p.
