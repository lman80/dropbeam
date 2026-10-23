// Audio/video calls — the desktop call screen's engine (docs/CALLS.md).
//
// WebRTC runs here, in the webview: echo cancellation, noise suppression, Opus,
// jitter buffer, bandwidth estimation, H.264/VP8. Its ONLY ICE server is the
// TURN bridge DropBeam runs on 127.0.0.1 for this call (relay policy), and the
// bridge carries every packet inside the friend's iroh connection. Signaling
// (offer/answer, ringing, recording consent) goes through Rust commands; the
// engine checks every description (fingerprint present, only virtual relay
// candidates) and tunes Opus before it leaves this device.

import { invoke } from '@tauri-apps/api/core'
import { listen, type UnlistenFn } from '@tauri-apps/api/event'
import { create } from 'zustand'
import { HAS_TAURI } from './api'
import { IS_MAC } from './platform'

export type CallPhase = 'ringing' | 'connecting' | 'active' | 'ended'
export type CallEnd = 'hangup' | 'remote_hangup' | 'declined' | 'busy' | 'no_answer' | 'missed' | 'canceled' | 'answered_elsewhere' | 'unavailable' | 'failed'

export interface RecordingConsent {
  askedByMe: boolean
  theyAllow: boolean
  askedByThem: boolean
  iAllow: boolean
  meRecording: boolean
  themRecording: boolean
  iShare: boolean
  theyShare: boolean
}

export interface CallSnapshot {
  id: string
  dir: 'out' | 'in'
  /** Friend id that owns this person's chat thread. */
  person: string
  name: string
  video: boolean
  phase: CallPhase
  startedMs: number
  answeredMs: number | null
  mediaMs: number | null
  endedMs: number | null
  end: CallEnd | null
  legs: { endpoint: string; state: string }[]
  active: string | null
  autoAccept: boolean
  glare: boolean
  /** A device of theirs said it can't take calls. */
  unsupported?: boolean
  recording: RecordingConsent
}

interface TurnInfo { urls: string[]; username: string; credential: string }

export interface CallQuality {
  /** local / direct / relay — iroh's path for the call. */
  path: string | null
  /** Media round trip, ms (WebRTC's own measurement through the bridge). */
  rttMs: number | null
  /** Packets lost in the last interval, percent (incoming audio). */
  lossPct: number | null
  jitterMs: number | null
  /** Incoming audio jitter-buffer delay (ms) over the last interval. */
  jbMs: number | null
  /** Outgoing media bitrate, kbps. */
  sendKbps: number | null
  recvKbps: number | null
  video: { width: number; height: number; fps: number } | null
  codec: string | null
  /** 'good' | 'ok' | 'poor' */
  grade: 'good' | 'ok' | 'poor' | null
}

const api = {
  capable: (supported: boolean) => invoke<void>('call_capable', { supported }),
  start: (friendId: string, video: boolean) => invoke<{ callId: string; turn: TurnInfo; call: CallSnapshot }>('call_start', { friendId, video }),
  offer: (callId: string, sdp: string) => invoke<void>('call_offer', { callId, sdp }),
  accept: (callId: string) => invoke<{ callId: string; turn: TurnInfo; sdp: string; video: boolean; name: string }>('call_accept', { callId }),
  answer: (callId: string, sdp: string) => invoke<void>('call_answer', { callId, sdp }),
  decline: (callId: string) => invoke<void>('call_decline', { callId }),
  hangup: (callId: string) => invoke<void>('call_hangup', { callId }),
  mediaUp: (callId: string) => invoke<void>('call_media_up', { callId }),
  signal: (callId: string, frame: Record<string, unknown>) => invoke<void>('call_signal', { callId, frame }),
  status: () => invoke<{ call: CallSnapshot | null; last: CallSnapshot | null; capable: boolean; lab?: { fakeMedia: boolean; autoAnswer: boolean } }>('call_status'),
  stats: (callId: string) => invoke<{ path: { path: string; rttMs: number | null } | null; bridge: Record<string, number> | null }>('call_stats', { callId }),
  recordOpen: (callId: string) => invoke<{ recId: string; dir: string }>('call_record_open', { callId }),
  recordChunk: (recId: string, track: string, ext: string, bytes: Uint8Array) =>
    invoke<void>('call_record_chunk', bytes, { headers: { 'x-rec-id': recId, 'x-track': track, 'x-ext': ext } }),
  recordClose: (recId: string) => invoke<{ dir: string }>('call_record_close', { recId }),
}

/** Can this webview make calls? (WebKitGTK builds often ship without WebRTC.) */
export function callsSupported(): boolean {
  return typeof window !== 'undefined' && typeof RTCPeerConnection !== 'undefined' && (!!navigator.mediaDevices?.getUserMedia || !!lab?.fakeMedia)
}

export const CALLS_UNSUPPORTED = 'Calls need a newer system web view on this computer (WebRTC isn’t available here).'

interface CallUi {
  call: CallSnapshot | null
  /** The ended call, kept for the "Call ended" card for a moment. */
  last: CallSnapshot | null
  local: MediaStream | null
  remote: MediaStream | null
  muted: boolean
  cameraOn: boolean
  remoteMuted: boolean
  /** null until the other side says (then follows their camera button). */
  remoteCamera: boolean | null
  quality: CallQuality | null
  recording: { recId: string; dir: string } | null
  savedTo: string | null
  error: string | null
  busy: boolean
  /** Swap clean voice tracks after the call (both must tick it). */
  shareTracks: boolean
}

export const useCall = create<CallUi>(() => ({
  call: null, last: null, local: null, remote: null, muted: false, cameraOn: false, remoteMuted: false, remoteCamera: null,
  quality: null, recording: null, savedTo: null, error: null, busy: false, shareTracks: false,
}))

const set = useCall.setState

// ── engine state (one call at a time) ──
let pc: RTCPeerConnection | null = null
let callId: string | null = null
let statsTimer: number | null = null
let lastBytes: { at: number; sent: number; recv: number; lost: number; got: number; jbd: number; jbn: number } | null = null
let recorders: MediaRecorder[] = []
let mixCtx: AudioContext | null = null
let pendingAnswer: string | null = null

function gathered(p: RTCPeerConnection): Promise<void> {
  return new Promise((resolve) => {
    if (p.iceGatheringState === 'complete') return resolve()
    const done = () => { if (p.iceGatheringState === 'complete') { p.removeEventListener('icegatheringstatechange', done); resolve() } }
    p.addEventListener('icegatheringstatechange', done)
    // Every candidate comes from the local TURN bridge: this is fast. Don't
    // wait forever on an interface that never answers.
    window.setTimeout(resolve, 3000)
  })
}

const AUDIO: MediaTrackConstraints = { echoCancellation: true, noiseSuppression: true, autoGainControl: true, channelCount: 1, sampleRate: 48000 }
const VIDEO: MediaTrackConstraints = { width: { ideal: 1280 }, height: { ideal: 720 }, frameRate: { ideal: 30 } }

/** Debug builds only (env DROPBEAM_CALL_FAKE_MEDIA / _AUTOANSWER): test media + delay probe. */
let lab: { fakeMedia: boolean; autoAnswer: boolean } | null = null
let stopProbe: (() => void) | null = null

async function getMedia(video: boolean): Promise<MediaStream> {
  if (lab?.fakeMedia) return (await import('./callLab')).fakeMedia(video)
  try {
    return await navigator.mediaDevices.getUserMedia({ audio: AUDIO, video: video ? VIDEO : false })
  } catch (e) {
    if (video) {
      // No camera (or it's busy): the call still goes ahead with sound.
      try { return await navigator.mediaDevices.getUserMedia({ audio: AUDIO, video: false }) } catch { /* fall through */ }
    }
    throw new Error(String(e).includes('NotAllowed') || String(e).includes('Permission')
      ? 'DropBeam needs the microphone to call. Allow it in System Settings → Privacy & Security → Microphone.'
      : `Couldn’t open the microphone: ${String(e)}`, { cause: e })
  }
}

function makePc(turn: TurnInfo): RTCPeerConnection {
  const p = new RTCPeerConnection({
    iceServers: [{ urls: turn.urls, username: turn.username, credential: turn.credential }],
    // Every packet goes through DropBeam's bridge — never straight onto the network.
    iceTransportPolicy: 'relay',
    bundlePolicy: 'max-bundle',
    rtcpMuxPolicy: 'require',
  })
  p.addEventListener('track', (e) => {
    const cur = useCall.getState().remote ?? new MediaStream()
    if (!cur.getTracks().includes(e.track)) cur.addTrack(e.track)
    const remote = new MediaStream(cur.getTracks())
    set({ remote })
    if (lab?.fakeMedia && !stopProbe && remote.getAudioTracks().length && remote.getVideoTracks().length) startProbe(remote)
  })
  p.addEventListener('connectionstatechange', () => {
    if (!callId || p !== pc) return
    if (p.connectionState === 'connected') {
      void api.mediaUp(callId).catch(() => {})
      const st = useCall.getState()
      void api.signal(callId, { kind: 'call.media', muted: st.muted, camera: st.cameraOn }).catch(() => {})
    }
    if (p.connectionState === 'failed') void hangup()
  })
  return p
}

/** H.264 first on Apple hardware (hardware encoder), else the browser's order. */
function preferCodecs(p: RTCPeerConnection) {
  if (!IS_MAC || typeof RTCRtpReceiver === 'undefined' || !RTCRtpReceiver.getCapabilities) return
  const caps = RTCRtpReceiver.getCapabilities('video')?.codecs ?? []
  const h264 = caps.filter((c) => /h264/i.test(c.mimeType))
  if (!h264.length) return
  const rest = caps.filter((c) => !/h264/i.test(c.mimeType))
  for (const t of p.getTransceivers()) {
    if (t.receiver.track?.kind === 'video') { try { t.setCodecPreferences([...h264, ...rest]) } catch { /* unsupported */ } }
  }
}

/** Opus up to 128 kbps; video up to 2.5 Mbps at 720p (more is wasted on a call). */
async function tuneSenders(p: RTCPeerConnection) {
  for (const s of p.getSenders()) {
    const kind = s.track?.kind ?? (p.getTransceivers().find((t) => t.sender === s)?.receiver.track?.kind)
    if (!kind) continue
    const params = s.getParameters()
    if (!params.encodings?.length) params.encodings = [{}]
    const enc = params.encodings[0] as RTCRtpEncodingParameters & { networkPriority?: string; priority?: string }
    if (kind === 'audio') { enc.maxBitrate = 128_000; enc.priority = 'high'; enc.networkPriority = 'high' }
    else { enc.maxBitrate = 2_500_000; enc.maxFramerate = 30 }
    try { await s.setParameters(params) } catch { /* older engines */ }
  }
}

/** Audio and video transceivers always exist, so the camera can come on later
 *  in an audio call without renegotiating. */
function addMedia(p: RTCPeerConnection, local: MediaStream) {
  const a = local.getAudioTracks()[0]
  const v = local.getVideoTracks()[0]
  const existing = p.getTransceivers()
  const at = existing.find((t) => t.receiver.track?.kind === 'audio')
  const vt = existing.find((t) => t.receiver.track?.kind === 'video')
  if (at) { void at.sender.replaceTrack(a ?? null); at.direction = 'sendrecv' } else p.addTransceiver(a ?? 'audio', { direction: 'sendrecv', streams: [local] })
  if (vt) { void vt.sender.replaceTrack(v ?? null); vt.direction = 'sendrecv' } else p.addTransceiver(v ?? 'video', { direction: 'sendrecv', streams: [local] })
  if (v) v.contentHint = 'motion'
}

function startProbe(remote: MediaStream) {
  stopProbe = () => {}
  void import('./callLab').then((m) => {
    stopProbe = m.probe(remote, (r) => {
      const q = useCall.getState().quality
      void invoke('frontend_log', { msg: `call-lab ${JSON.stringify({ ...r, rttMs: q?.rttMs, lossPct: q?.lossPct, jitterMs: q?.jitterMs, jbMs: q?.jbMs, sendKbps: q?.sendKbps, recvKbps: q?.recvKbps, video: q?.video, codec: q?.codec, path: q?.path })}` }).catch(() => {})
    })
  })
}

function reset() {
  stopProbe?.()
  stopProbe = null
  if (statsTimer) window.clearInterval(statsTimer)
  statsTimer = null
  lastBytes = null
  pendingAnswer = null
  try { pc?.close() } catch { /* already closed */ }
  pc = null
  callId = null
  stopRecording(false)
  for (const t of useCall.getState().local?.getTracks() ?? []) t.stop()
  stopRing()
  set({ local: null, remote: null, muted: false, cameraOn: false, remoteMuted: false, remoteCamera: null, quality: null, busy: false })
}

// ── placing and answering ──

export async function startCall(friendId: string, video: boolean): Promise<void> {
  if (!callsSupported()) throw new Error(CALLS_UNSUPPORTED)
  if (useCall.getState().call && useCall.getState().call?.phase !== 'ended') throw new Error('You’re already in a call.')
  set({ busy: true, error: null, savedTo: null, last: null })
  let local: MediaStream | null = null
  try {
    local = await getMedia(video)
    const started = await api.start(friendId, video)
    callId = started.callId
    set({ call: started.call, local, cameraOn: local.getVideoTracks().length > 0 })
    pc = makePc(started.turn)
    addMedia(pc, local)
    preferCodecs(pc)
    await tuneSenders(pc)
    await pc.setLocalDescription(await pc.createOffer())
    await gathered(pc)
    await api.offer(started.callId, pc.localDescription!.sdp)
    startStats()
  } catch (e) {
    const id = callId
    for (const t of local?.getTracks() ?? []) t.stop()
    if (id) void api.hangup(id).catch(() => {})
    reset()
    set({ busy: false, error: String(e) })
    throw e
  } finally {
    set({ busy: false })
  }
}

export async function acceptCall(): Promise<void> {
  const call = useCall.getState().call
  if (!call || call.dir !== 'in' || call.phase !== 'ringing') return
  stopRing()
  set({ busy: true, error: null })
  try {
    const acc = await api.accept(call.id)
    callId = call.id
    const local = await getMedia(acc.video)
    set({ local, cameraOn: local.getVideoTracks().length > 0 })
    pc = makePc(acc.turn)
    await pc.setRemoteDescription({ type: 'offer', sdp: acc.sdp })
    addMedia(pc, local)
    await tuneSenders(pc)
    await pc.setLocalDescription(await pc.createAnswer())
    await gathered(pc)
    await api.answer(call.id, pc.localDescription!.sdp)
    startStats()
  } catch (e) {
    void api.decline(call.id).catch(() => {})
    reset()
    set({ error: String(e) })
  } finally {
    set({ busy: false })
  }
}

export function declineCall() {
  const call = useCall.getState().call
  stopRing()
  if (call) void api.decline(call.id).catch(() => {})
}

export async function hangup() {
  const id = callId ?? useCall.getState().call?.id
  if (id) await api.hangup(id).catch(() => {})
}

// ── in-call controls ──

export function toggleMute() {
  const s = useCall.getState()
  const muted = !s.muted
  for (const t of s.local?.getAudioTracks() ?? []) t.enabled = !muted
  set({ muted })
  if (callId) void api.signal(callId, { kind: 'call.media', muted, camera: s.cameraOn }).catch(() => {})
}

export async function toggleCamera() {
  const s = useCall.getState()
  if (!pc || !s.local) return
  const vt = pc.getTransceivers().find((t) => t.receiver.track?.kind === 'video')
  if (s.cameraOn) {
    for (const t of s.local.getVideoTracks()) { t.stop(); s.local.removeTrack(t) }
    await vt?.sender.replaceTrack(null)
    set({ cameraOn: false, local: new MediaStream(s.local.getTracks()) })
  } else {
    try {
      const cam = await navigator.mediaDevices.getUserMedia({ video: VIDEO })
      const track = cam.getVideoTracks()[0]
      track.contentHint = 'motion'
      await vt?.sender.replaceTrack(track)
      s.local.addTrack(track)
      set({ cameraOn: true, local: new MediaStream(s.local.getTracks()) })
    } catch (e) {
      set({ error: `Couldn’t turn on the camera: ${String(e)}` })
      return
    }
  }
  if (callId) void api.signal(callId, { kind: 'call.media', muted: s.muted, camera: !s.cameraOn }).catch(() => {})
}

export async function cameras(): Promise<MediaDeviceInfo[]> {
  return (await navigator.mediaDevices.enumerateDevices()).filter((d) => d.kind === 'videoinput')
}
export async function speakers(): Promise<MediaDeviceInfo[]> {
  return (await navigator.mediaDevices.enumerateDevices()).filter((d) => d.kind === 'audiooutput')
}

/** Next camera (front/back, built-in/external). */
export async function switchCamera() {
  const s = useCall.getState()
  if (!pc || !s.local || !s.cameraOn) return
  const list = await cameras()
  if (list.length < 2) return
  const cur = s.local.getVideoTracks()[0]?.getSettings().deviceId
  const next = list[(list.findIndex((d) => d.deviceId === cur) + 1) % list.length]
  const cam = await navigator.mediaDevices.getUserMedia({ video: { ...VIDEO, deviceId: { exact: next.deviceId } } })
  const track = cam.getVideoTracks()[0]
  const vt = pc.getTransceivers().find((t) => t.receiver.track?.kind === 'video')
  await vt?.sender.replaceTrack(track)
  for (const t of s.local.getVideoTracks()) { t.stop(); s.local.removeTrack(t) }
  s.local.addTrack(track)
  set({ local: new MediaStream(s.local.getTracks()) })
}

// ── quality ──

function startStats() {
  if (statsTimer) window.clearInterval(statsTimer)
  statsTimer = window.setInterval(() => { void sampleStats() }, 2000)
}

async function sampleStats() {
  if (!pc || !callId) return
  const r = await pc.getStats().catch(() => null)
  if (!r) return
  let rtt: number | null = null, sent = 0, recv = 0, lost = 0, got = 0, jitter: number | null = null, jbd = 0, jbn = 0
  let video: CallQuality['video'] = null, codecId: string | null = null
  const codecs = new Map<string, string>()
  r.forEach((s: Record<string, unknown> & { type: string; id: string }) => {
    if (s.type === 'candidate-pair' && s.state === 'succeeded' && (s.nominated || s.selected) && typeof s.currentRoundTripTime === 'number') rtt = Math.round((s.currentRoundTripTime as number) * 1000)
    if (s.type === 'outbound-rtp') sent += (s.bytesSent as number) || 0
    if (s.type === 'inbound-rtp') {
      recv += (s.bytesReceived as number) || 0
      if (s.kind === 'audio') { lost += (s.packetsLost as number) || 0; got += (s.packetsReceived as number) || 0; jitter = Math.round(((s.jitter as number) || 0) * 1000); jbd += (s.jitterBufferDelay as number) || 0; jbn += (s.jitterBufferEmittedCount as number) || 0 }
      if (s.kind === 'video' && s.frameWidth) { video = { width: s.frameWidth as number, height: s.frameHeight as number, fps: Math.round((s.framesPerSecond as number) || 0) }; codecId = (s.codecId as string) ?? null }
    }
    if (s.type === 'codec') codecs.set(s.id, String(s.mimeType ?? ''))
  })
  const now = performance.now()
  let sendKbps: number | null = null, recvKbps: number | null = null, lossPct: number | null = null, jbMs: number | null = null
  if (lastBytes) {
    if (jbn > lastBytes.jbn) jbMs = Math.round((1000 * (jbd - lastBytes.jbd)) / (jbn - lastBytes.jbn))
    const secs = (now - lastBytes.at) / 1000
    sendKbps = Math.round(((sent - lastBytes.sent) * 8) / 1000 / secs)
    recvKbps = Math.round(((recv - lastBytes.recv) * 8) / 1000 / secs)
    const dl = lost - lastBytes.lost, dg = got - lastBytes.got
    lossPct = dl + dg > 0 ? Math.max(0, Math.round((1000 * dl) / (dl + dg)) / 10) : 0
  }
  lastBytes = { at: now, sent, recv, lost, got, jbd, jbn }
  const engine = await api.stats(callId).catch(() => null)
  const path = engine?.path?.path ?? null
  const rttMs: number | null = rtt ?? engine?.path?.rttMs ?? null
  const grade: CallQuality['grade'] = rttMs == null ? null : rttMs > 400 || (lossPct ?? 0) > 8 ? 'poor' : rttMs > 200 || (lossPct ?? 0) > 3 ? 'ok' : 'good'
  set({ quality: { path, rttMs, lossPct, jitterMs: jitter, jbMs, sendKbps, recvKbps, video, codec: codecId ? (codecs.get(codecId) ?? '').replace(/^video\//, '') || null : null, grade } })
}

// ── recording (on this device, with the other person's yes) ──

export function askToRecord() {
  if (callId) void api.signal(callId, { kind: 'call.record', action: 'ask' }).catch((e) => set({ error: String(e) }))
}
export function answerRecordRequest(allow: boolean) {
  if (callId) void api.signal(callId, { kind: 'call.record', action: allow ? 'allow' : 'deny' }).catch((e) => set({ error: String(e) }))
}
export function offerTrackSwap() {
  set({ shareTracks: true })
  if (callId) void api.signal(callId, { kind: 'call.record', action: 'share' }).catch(() => {})
}

function pickType(video: boolean): { mime: string; ext: string } {
  const opts = video
    ? [['video/webm;codecs=vp8,opus', 'webm'], ['video/webm', 'webm'], ['video/mp4', 'mp4']]
    : [['audio/webm;codecs=opus', 'webm'], ['audio/mp4', 'm4a'], ['audio/ogg;codecs=opus', 'ogg'], ['audio/webm', 'webm']]
  for (const [mime, ext] of opts) if (typeof MediaRecorder !== 'undefined' && MediaRecorder.isTypeSupported(mime)) return { mime, ext }
  return { mime: '', ext: video ? 'webm' : 'webm' }
}

function recordTrack(recId: string, track: 'me' | 'them' | 'mixed', stream: MediaStream, video: boolean) {
  const { mime, ext } = pickType(video)
  const rec = new MediaRecorder(stream, mime ? { mimeType: mime, audioBitsPerSecond: 128_000 } : undefined)
  // Chunks are written in order; a queue keeps a slow write from reordering them.
  let chain = Promise.resolve()
  rec.ondataavailable = (e) => {
    if (!e.data.size) return
    chain = chain.then(async () => { await api.recordChunk(recId, track, ext, new Uint8Array(await e.data.arrayBuffer())) }).catch(() => {})
  }
  ;(rec as MediaRecorder & { done?: Promise<void> }).done = new Promise<void>((resolve) => { rec.onstop = () => { void chain.then(() => resolve()) } })
  rec.start(1000)
  recorders.push(rec)
}

export async function startRecording() {
  const s = useCall.getState()
  if (!callId || !s.local || !s.remote || recorders.length) return
  try {
    const opened = await api.recordOpen(callId)
    set({ recording: opened, savedTo: null })
    const me = new MediaStream(s.local.getAudioTracks())
    const them = new MediaStream(s.remote.getAudioTracks())
    recordTrack(opened.recId, 'me', me, false)
    recordTrack(opened.recId, 'them', them, false)
    // The mixed call: both voices into one track, plus their video if any.
    mixCtx = new AudioContext({ sampleRate: 48000 })
    const dest = mixCtx.createMediaStreamDestination()
    if (me.getAudioTracks().length) mixCtx.createMediaStreamSource(me).connect(dest)
    if (them.getAudioTracks().length) mixCtx.createMediaStreamSource(them).connect(dest)
    const remoteVideo = s.remote.getVideoTracks().filter((t) => t.readyState === 'live')
    recordTrack(opened.recId, 'mixed', new MediaStream([...dest.stream.getAudioTracks(), ...remoteVideo]), remoteVideo.length > 0)
  } catch (e) {
    set({ error: String(e) })
  }
}

export function stopRecording(close = true) {
  const rec = useCall.getState().recording
  const all = recorders
  recorders = []
  for (const r of all) { try { r.stop() } catch { /* stopped */ } }
  void mixCtx?.close().catch(() => {})
  mixCtx = null
  if (!rec) return
  set({ recording: null })
  if (!close) {
    // The call ended: Rust already closed the files; flush what's queued.
    set({ savedTo: rec.dir })
    return
  }
  void Promise.all(all.map((r) => (r as MediaRecorder & { done?: Promise<void> }).done)).then(async () => {
    try { const out = await api.recordClose(rec.recId); set({ savedTo: out.dir }) } catch (e) { set({ error: String(e) }) }
  })
}

// ── ringing ──

let ringCtx: AudioContext | null = null
let ringTimer: number | null = null
function startRing(outgoing: boolean) {
  stopRing()
  try {
    ringCtx = new AudioContext()
    const play = () => {
      if (!ringCtx) return
      const t = ringCtx.currentTime
      const tones = outgoing ? [[440, 480]] : [[659, 784], [784, 988]]
      tones.forEach((pair, i) => {
        for (const f of pair) {
          const o = ringCtx!.createOscillator(), g = ringCtx!.createGain()
          o.frequency.value = f
          g.gain.setValueAtTime(0, t + i * 0.45)
          g.gain.linearRampToValueAtTime(outgoing ? 0.03 : 0.06, t + i * 0.45 + 0.03)
          g.gain.linearRampToValueAtTime(0, t + i * 0.45 + (outgoing ? 1.2 : 0.4))
          o.connect(g).connect(ringCtx!.destination)
          o.start(t + i * 0.45)
          o.stop(t + i * 0.45 + (outgoing ? 1.3 : 0.45))
        }
      })
    }
    play()
    ringTimer = window.setInterval(play, outgoing ? 4000 : 2500)
  } catch { /* no audio output */ }
}
function stopRing() {
  if (ringTimer) window.clearInterval(ringTimer)
  ringTimer = null
  void ringCtx?.close().catch(() => {})
  ringCtx = null
}

// ── events from the engine ──

let started = false
/** Main window only: report capability and follow the engine's call events. */
export function startCallEngine(onEnded?: (c: CallSnapshot) => void): () => void {
  if (!HAS_TAURI || started) return () => {}
  started = true
  void invoke('frontend_log', { msg: `calls: webrtc=${typeof RTCPeerConnection !== 'undefined'} media=${!!navigator.mediaDevices?.getUserMedia} recorder=${typeof MediaRecorder !== 'undefined'}` }).catch(() => {})
  void api.status().then((st) => { lab = st.lab ?? null }).catch(() => {}).finally(() => { void api.capable(callsSupported()).catch(() => {}) })
  const uns: Promise<UnlistenFn>[] = [
    listen<CallSnapshot>('call://state', (e) => onState(e.payload, onEnded)),
    listen<CallSnapshot>('call://incoming', (e) => {
      const c = e.payload
      set({ call: c, last: null, savedTo: null, error: null })
      if (c.autoAccept) void acceptCall()
      else startRing(false)
    }),
    listen<{ callId: string; kind: string; sdp?: string; body?: Record<string, unknown> }>('call://signal', (e) => void onSignal(e.payload)),
    listen<{ friendId: string; video: boolean }>('call://dial', (e) => { void startCall(e.payload.friendId, !!e.payload.video).catch(() => {}) }),
  ]
  // A call already in progress (the window reloaded): show its state.
  void api.status().then((s) => { if (s.call) set({ call: s.call }) }).catch(() => {})
  return () => { started = false; for (const u of uns) void u.then((f) => f()) }
}

function onState(c: CallSnapshot, onEnded?: (c: CallSnapshot) => void) {
  const prev = useCall.getState().call
  if (prev && prev.id !== c.id && prev.phase !== 'ended' && c.phase === 'ended') return
  if (c.phase === 'ended') {
    if (callId === c.id || !callId) {
      const share = useCall.getState().shareTracks && c.recording.theyShare
      const rec = useCall.getState().recording
      reset()
      if (rec) set({ savedTo: rec.dir })
      // Ended only because both called at once: the other call takes over.
      if (c.glare) { set({ call: null }); return }
      set({ call: null, last: c, shareTracks: false })
      if (share) set({ error: null })
      onEnded?.(c)
    }
    return
  }
  if (c.dir === 'out' && c.phase === 'ringing' && c.legs.some((l) => l.state === 'ringing') && !ringTimer) startRing(true)
  if (c.phase !== 'ringing') stopRing()
  set({ call: c })
  // Their recording consent arrived and I asked: start.
  if (c.recording.theyAllow && c.recording.askedByMe && !c.recording.meRecording && !recorders.length && c.phase === 'active') void startRecording()
}

async function onSignal(s: { callId: string; kind: string; sdp?: string; body?: Record<string, unknown> }) {
  if (s.callId !== callId && s.kind !== 'call.answer') return
  if (s.kind === 'call.answer' && s.sdp) {
    if (!pc) { pendingAnswer = s.sdp; return }
    await pc.setRemoteDescription({ type: 'answer', sdp: s.sdp }).catch((e) => set({ error: String(e) }))
  } else if (s.kind === 'call.media' && s.body) {
    set({ remoteMuted: !!s.body.muted, remoteCamera: !!s.body.camera })
  } else if (s.kind === 'call.record' && s.body?.action === 'deny') {
    set({ error: `${useCall.getState().call?.name ?? 'They'} didn’t allow recording.` })
  }
}

void pendingAnswer
