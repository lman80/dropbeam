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
| `call.glare` | we are calling you at this moment and our call wins |
| `call.hangup` | end |
| `call.media {muted, camera}` · `call.record {action}` · `call.sdp` | mid-call |

- **One call at a time.** A ring while busy is answered `call.busy` and logged as a
  missed call.
- **Several devices ring; the first to answer wins** — the others get `call.taken`
  (no history line on them). A decline on any device ends the call for all.
- **Glare** (both call each other at once): both sides compare the two call ids; the
  smaller one survives and is answered automatically on the other side.
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
  `calls.start` (rings from this computer's call screen; someone must be there to
  talk) and `calls.hangup`. Answering, recording and the WebRTC plumbing are
  person-only (EXCLUDED with reasons).

## Recording (on the device, with consent)

- **Record** on the call screen sends `call.record {action:"ask"}`. The other side sees
  "<Name> wants to record this call on their device — Allow / Don't allow".
- `call_record_open` is refused until an `allow` for this call arrived. The **engine**
  (not the screen) sends `started`/`stopped`, so the other side's red "Recording"
  badge can't be skipped by a modified call screen on the recorder's side.
- Desktop: MediaRecorder writes three files to
  `<Downloads>/DropBeam Calls/<date time> <Name>/`: `Me.webm` (your voice),
  `<Name>.webm` (their voice), `Call.webm` (both voices mixed, with their video when
  there was any) and `call.json` (participants, start/end, which file is whose).
  Nothing is uploaded anywhere.
- **Swap clean tracks** (for AI notes): while recording, either side can offer to swap;
  when both agreed (`call.record {action:"share"}` both ways), after the call each side
  sends its own `Me` track to the other through the normal verified file transfer, so
  each ends up with both clean voices.
- iOS: consent and the badge work; recording on the iPhone itself isn't built yet.

## Platforms

| | macOS | Windows | Linux | iOS |
|---|---|---|---|---|
| Audio/video calls | ✅ tested (WKWebView) | ⚠️ builds in CI; WebView2 has WebRTC, not call-tested | ❌ Ubuntu's WebKitGTK ships without WebRTC (checked 2.52 on 24.04: `RTCPeerConnection` is undefined) → "can't take calls yet" | ⚠️ builds + launches in the simulator; not call-tested |
| Ringing while hidden/in tray | ✅ window comes up + notification | ✅ same code | — | CallKit; VoIP push when backgrounded (owner setup) |
| Recording | ✅ (desktop code; mic recording needs a real mic) | ⚠️ | — | consent only |

Linux shows the call buttons and explains why a call can't start; friends calling a
Linux device read "<Name>'s device can't take calls yet". Options for later: native
libwebrtc (LiveKit's `webrtc-sys`, BSD) driven from Rust with the same TURN bridge.

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
two devices of one person (first answer wins, the other stops); glare.

`calls::lab_tests` (`#[ignore]`, needs Chrome): real WebRTC in headless Chrome with fake
camera/mic, two peer connections each limited to its own bridge, bridges joined by a
loopback iroh connection.

Live (debug builds; `DROPBEAM_CALL_FAKE_MEDIA=1` gives a synthetic camera/mic carrying
the sender's clock, so the receiver measures delay; `dropbeam-lab call-node` runs the
engine headless with a Chrome call screen). One-way delays across machines are the
average of both directions (cancels clock offset; NTP-synced machines).

| Run | Path | Setup to media | Media RTT | Video (glass→glass) | Audio (mouth→ear, before the sound card) | Loss | Video |
|---|---|---|---|---|---|---|---|
| ass-box, loopback, 2× Chrome | local | 1.0–1.3 s | 3–19 ms | — | — | 0 % | 720p |
| This Mac, 2 app instances (WKWebView) | local | 0.5 s (answer→media 40 ms) | 1–3 ms | 40–55 ms | jitter buffer grew to ~350 ms ¹ | 0 % | 720p30 H.264 |
| Mac (Wi-Fi) ↔ ass-box, same home LAN | local | — | 22–45 ms (Wi-Fi ping 9–97 ms) | ~250 ms ² | ~230 ms (jitter buffer ~130 ms) | 0 % | 720p |
| linux-agent ↔ ass-box (across town) | direct | 1.4 s | 37–47 ms | — ² | ~157 ms (one way, both directions averaged) | ≤0.5 % | 720p one way, 270p other ² |
| Mac (Wi-Fi) ↔ Korea (minkyung-mac) | direct (hole-punched) | — | 240–345 ms (iroh RTT 230–250) | ~610 ms ² | ~390 ms (jitter buffer 260–410) | ≤7 % | 720p / 360p |
| linux-agent ↔ ass-box, relay forced | relay | see below | | | | | |

¹ Synthetic audio in WKWebView (a Web Audio track) — real microphones take WebKit's
capture path; the video on the same call stayed at ~50 ms. ² ass-box (4 cores) encoding
720p in software in headless Chrome plus a debug engine was CPU-bound (load 3–4.5);
video delay grew with it. Same Wi-Fi caveat on the Mac. These are debug builds.

**Reading**: the bridge adds no measurable delay (media RTT ≈ iroh RTT ≈ ping); delay
above that is WebRTC's jitter buffer reacting to network jitter (home Wi-Fi), CPU, and
distance. On a quiet LAN audio is well under 150 ms one way; across town ~150 ms; US ↔
Korea ~400 ms.

**Not verified**: a real camera and microphone (macOS permission prompts need a person),
Windows calls, an iPhone (real device, CallKit, VoIP push), recording with a real
microphone (consent flow is tested in code; the MediaRecorder path in a real call is
not), echo cancellation quality, 1080p.
