// The call screen (desktop main window): ringing, the call itself, recording
// consent, and the "call ended" card. The engine is src/lib/calls.ts.

import { useEffect, useMemo, useRef, useState } from 'react'
import {
  Circle, Mic, MicOff, Minimize2, Maximize2, Phone, PhoneOff, RefreshCw, Video, VideoOff, Volume2,
} from 'lucide-react'
import { api } from '../lib/api'
import {
  acceptCall, answerRecordRequest, askToRecord, cameras, declineCall, hangup, offerTrackSwap, speakers,
  startCallEngine, stopRecording, switchCamera, toggleCamera, toggleMute, useCall, type CallSnapshot,
} from '../lib/calls'
import { useStore } from '../store'
import { avatarGradient } from '../lib/avatar'
import { FriendAvatar } from './FriendAvatar'

function clock(ms: number): string {
  const s = Math.max(0, Math.floor(ms / 1000))
  const h = Math.floor(s / 3600), m = Math.floor((s / 60) % 60), sec = s % 60
  return h ? `${h}:${String(m).padStart(2, '0')}:${String(sec).padStart(2, '0')}` : `${m}:${String(sec).padStart(2, '0')}`
}

const PATH_WORDS: Record<string, string> = { local: 'Same network', direct: 'Direct', relay: 'Relay', connecting: 'Connecting' }

function useNow(active: boolean) {
  const [now, setNow] = useState(() => Date.now())
  useEffect(() => {
    if (!active) return
    const t = window.setInterval(() => setNow(Date.now()), 500)
    return () => window.clearInterval(t)
  }, [active])
  return now
}

function MediaView({ stream, muted, mirror, className }: { stream: MediaStream | null; muted?: boolean; mirror?: boolean; className?: string }) {
  const ref = useRef<HTMLVideoElement>(null)
  useEffect(() => {
    const v = ref.current
    if (!v) return
    if (v.srcObject !== stream) v.srcObject = stream
    if (stream) void v.play().catch(() => {})
  }, [stream])
  return <video ref={ref} className={className} autoPlay playsInline muted={muted} style={mirror ? { transform: 'scaleX(-1)' } : undefined} />
}

function statusLine(c: CallSnapshot, now: number): string {
  if (c.phase === 'ringing') {
    if (c.dir === 'in') return c.video ? 'Video call' : 'Audio call'
    return c.legs.some((l) => l.state === 'ringing') ? 'Ringing…' : 'Calling…'
  }
  if (c.phase === 'connecting') return 'Connecting…'
  return clock(now - (c.mediaMs ?? c.answeredMs ?? now))
}

function endedLine(c: CallSnapshot): string {
  const dur = c.mediaMs && c.endedMs ? clock(c.endedMs - c.mediaMs) : null
  switch (c.end) {
    case 'hangup': case 'remote_hangup': return dur ? `Call ended · ${dur}` : 'Call ended'
    case 'declined': return c.dir === 'out' ? `${c.name} declined` : 'Declined'
    case 'busy': return `${c.name} is on another call`
    case 'no_answer': return 'No answer'
    case 'missed': return 'Missed call'
    case 'canceled': return 'Canceled'
    case 'unavailable': return `Couldn’t reach ${c.name}. They may need to open or update DropBeam.`
    case 'failed': return dur ? `Call dropped · ${dur}` : 'Couldn’t connect the call'
    default: return 'Call ended'
  }
}

export function CallHost() {
  const call = useCall((s) => s.call)
  const last = useCall((s) => s.last)
  const error = useCall((s) => s.error)
  const toast = useStore((s) => s.toast)

  useEffect(() => startCallEngine(), [])
  useEffect(() => {
    if (!error) return
    toast('error', error)
    useCall.setState({ error: null })
  }, [error, toast])

  if (call && call.dir === 'in' && call.phase === 'ringing' && !call.autoAccept) return <Incoming call={call} />
  if (call) return <InCall call={call} />
  if (last) return <Ended call={last} />
  return null
}

function Avatar({ person, name, size = 96 }: { person: string; name: string; size?: number }) {
  const friend = useStore((s) => s.friends.find((f) => f.id === person))
  return (
    <span className="call-avatar" style={{ width: size, height: size, background: avatarGradient(person) }}>
      <FriendAvatar friend={friend ?? { name, avatar: null }} />
    </span>
  )
}

function Incoming({ call }: { call: CallSnapshot }) {
  const busy = useCall((s) => s.busy)
  return (
    <div className="call-incoming" role="alertdialog" aria-modal="true" aria-label={`${call.name} is calling`}>
      <Avatar person={call.person} name={call.name} size={64} />
      <div className="call-incoming-text">
        <div className="call-incoming-name">{call.name}</div>
        <div className="call-incoming-sub">{call.video ? 'Video call' : 'Audio call'}</div>
      </div>
      <div className="call-incoming-actions">
        <button type="button" className="call-round call-decline" aria-label="Decline" onClick={declineCall} disabled={busy}><PhoneOff /></button>
        <button type="button" className="call-round call-accept" aria-label="Answer" onClick={() => void acceptCall()} disabled={busy} autoFocus>
          {call.video ? <Video /> : <Phone />}
        </button>
      </div>
    </div>
  )
}

function InCall({ call }: { call: CallSnapshot }) {
  const { local, remote, muted, cameraOn, remoteMuted, remoteCamera, quality, recording, shareTracks } = useCall()
  const [mini, setMini] = useState(false)
  const [cams, setCams] = useState(0)
  const [outs, setOuts] = useState<MediaDeviceInfo[]>([])
  const [sink, setSink] = useState('')
  const now = useNow(true)
  const remoteVideo = useMemo(() => !!remote?.getVideoTracks().some((t) => t.readyState === 'live') && remoteCamera !== false, [remote, remoteCamera])
  const showRemoteVideo = remoteVideo && (call.video || remoteCamera)
  const rec = call.recording
  const live = call.phase === 'active'

  useEffect(() => {
    void cameras().then((c) => setCams(c.length)).catch(() => {})
    void speakers().then(setOuts).catch(() => {})
  }, [cameraOn])

  // Remote audio plays through the video element; choose the speaker.
  const remoteRef = useRef<HTMLDivElement>(null)
  useEffect(() => {
    const v = remoteRef.current?.querySelector('video') as (HTMLVideoElement & { setSinkId?: (id: string) => Promise<void> }) | null
    if (sink && v?.setSinkId) void v.setSinkId(sink).catch(() => {})
  }, [sink, remote])

  const recBadge = rec.meRecording && rec.themRecording ? 'You’re both recording'
    : rec.meRecording ? 'Recording on this computer'
    : rec.themRecording ? `${call.name} is recording` : null

  return (
    <div className={mini ? 'call-screen mini' : 'call-screen'} role="dialog" aria-label={`Call with ${call.name}`}>
      <div className="call-stage" ref={remoteRef}>
        <MediaView stream={remote} className={showRemoteVideo ? 'call-remote' : 'call-remote hidden'} />
        {!showRemoteVideo && (
          <div className="call-who">
            <Avatar person={call.person} name={call.name} size={mini ? 40 : 112} />
            {!mini && <div className="call-who-name">{call.name}</div>}
            {!mini && <div className="call-who-sub">{statusLine(call, now)}{remoteMuted && live ? ' · muted' : ''}</div>}
          </div>
        )}
        {local && cameraOn && <MediaView stream={local} muted mirror className="call-local" />}
      </div>

      <div className="call-top">
        <div className="call-top-name">
          {showRemoteVideo && <span>{call.name}</span>}
          {showRemoteVideo && <span className="call-top-sub">{statusLine(call, now)}{remoteMuted ? ' · muted' : ''}</span>}
        </div>
        {recBadge && <span className="call-rec-badge" role="status"><Circle className="call-rec-dot" />{recBadge}</span>}
        {live && quality && (
          <span className={`call-quality ${quality.grade ?? ''}`} title={[
            quality.path && `Path: ${PATH_WORDS[quality.path] ?? quality.path}`,
            quality.rttMs != null && `Round trip: ${quality.rttMs} ms`,
            quality.lossPct != null && `Loss: ${quality.lossPct}%`,
            quality.jitterMs != null && `Jitter: ${quality.jitterMs} ms`,
            quality.sendKbps != null && `Sending: ${quality.sendKbps} kbps`,
            quality.recvKbps != null && `Receiving: ${quality.recvKbps} kbps`,
            quality.video && `Video: ${quality.video.width}×${quality.video.height} at ${quality.video.fps} fps${quality.codec ? ` (${quality.codec})` : ''}`,
          ].filter(Boolean).join('\n')}>
            <span className="call-bars" aria-hidden><i /><i /><i /></span>
            {quality.path ? PATH_WORDS[quality.path] ?? quality.path : ''}{quality.rttMs != null ? ` · ${quality.rttMs} ms` : ''}
          </span>
        )}
        <button type="button" className="call-icon" aria-label={mini ? 'Bigger' : 'Smaller'} onClick={() => setMini(!mini)}>
          {mini ? <Maximize2 /> : <Minimize2 />}
        </button>
      </div>

      {!mini && rec.askedByThem && !rec.iAllow && (
        <div className="call-banner" role="alertdialog" aria-label="Recording request">
          <span>{call.name} wants to record this call on their device.</span>
          <button type="button" className="btn btn-secondary btn-sm" onClick={() => answerRecordRequest(false)}>Don’t allow</button>
          <button type="button" className="btn btn-primary btn-sm" onClick={() => answerRecordRequest(true)}>Allow</button>
        </div>
      )}
      {!mini && rec.askedByMe && !rec.theyAllow && !rec.meRecording && (
        <div className="call-banner"><span>Waiting for {call.name} to allow recording…</span></div>
      )}
      {!mini && (rec.meRecording || recording) && !shareTracks && (
        <div className="call-banner subtle">
          <span>After the call, swap clean voice tracks so you both have both sides?</span>
          <button type="button" className="btn btn-secondary btn-sm" onClick={offerTrackSwap}>{rec.theyShare ? `${call.name} said yes — Swap` : 'Offer to swap'}</button>
        </div>
      )}

      <div className="call-controls">
        <button type="button" className={muted ? 'call-ctl on' : 'call-ctl'} aria-pressed={muted} aria-label={muted ? 'Unmute' : 'Mute'} onClick={toggleMute}>
          {muted ? <MicOff /> : <Mic />}<span>{muted ? 'Unmute' : 'Mute'}</span>
        </button>
        <button type="button" className={cameraOn ? 'call-ctl' : 'call-ctl on'} aria-pressed={!cameraOn} aria-label={cameraOn ? 'Turn camera off' : 'Turn camera on'} onClick={() => void toggleCamera()} disabled={!live && call.phase !== 'connecting' && call.dir === 'in'}>
          {cameraOn ? <Video /> : <VideoOff />}<span>Camera</span>
        </button>
        {!mini && cameraOn && cams > 1 && (
          <button type="button" className="call-ctl" aria-label="Switch camera" onClick={() => void switchCamera()}><RefreshCw /><span>Switch</span></button>
        )}
        {!mini && outs.length > 1 && 'setSinkId' in HTMLMediaElement.prototype && (
          <label className="call-ctl call-select" aria-label="Speaker">
            <Volume2 />
            <select value={sink} onChange={(e) => setSink(e.target.value)}>
              {outs.map((o) => <option key={o.deviceId} value={o.deviceId}>{o.label || 'Speaker'}</option>)}
            </select>
          </label>
        )}
        {!mini && live && (
          rec.meRecording || recording
            ? <button type="button" className="call-ctl on rec" aria-label="Stop recording" onClick={() => stopRecording(true)}><Circle /><span>Stop</span></button>
            : <button type="button" className="call-ctl" aria-label="Record" disabled={rec.askedByMe && !rec.theyAllow} onClick={askToRecord}><Circle /><span>Record</span></button>
        )}
        <button type="button" className="call-round call-decline" aria-label="End call" onClick={() => void hangup()}><PhoneOff /></button>
      </div>
    </div>
  )
}

function Ended({ call }: { call: CallSnapshot }) {
  const savedTo = useCall((s) => s.savedTo)
  const shareFilesInChat = useStore((s) => s.shareFilesInChat)
  const openChat = useStore((s) => s.openChat)
  const swapped = useRef(false)
  useEffect(() => {
    const t = window.setTimeout(() => useCall.setState({ last: null }), savedTo ? 15000 : 4000)
    return () => window.clearTimeout(t)
  }, [call.id, savedTo])
  // Both agreed to swap clean tracks: send mine through the normal verified transfer.
  useEffect(() => {
    if (swapped.current || !savedTo || !call.recording.iShare || !call.recording.theyShare) return
    swapped.current = true
    void (async () => {
      const recs = await invokeRecordings()
      const mine = recs.find((r) => r.dir === savedTo)?.meta?.tracks?.me?.file
      if (mine) await shareFilesInChat(call.person, [`${savedTo}/${mine}`], 'My side of our call (clean voice track)')
    })()
  }, [savedTo, call, shareFilesInChat])
  const redial = () => { useCall.setState({ last: null }); void import('../lib/calls').then((m) => m.startCall(call.person, call.video)).catch(() => {}) }
  return (
    <div className="call-ended" role="status">
      <Avatar person={call.person} name={call.name} size={36} />
      <div className="call-ended-text">
        <div className="call-ended-name">{call.name}</div>
        <div className="call-ended-sub">{endedLine(call)}</div>
        {savedTo && <button type="button" className="link-btn" onClick={() => void api.openPath(savedTo).catch(() => {})}>Recording saved — show it</button>}
      </div>
      {call.end !== 'answered_elsewhere' && (
        <button type="button" className="btn btn-secondary btn-sm" onClick={redial}>{call.video ? <Video size={14} /> : <Phone size={14} />} Call back</button>
      )}
      <button type="button" className="btn btn-secondary btn-sm" onClick={() => { useCall.setState({ last: null }); void openChat(call.person) }}>Message</button>
    </div>
  )
}

async function invokeRecordings(): Promise<{ dir: string; meta?: { tracks?: { me?: { file?: string } } } }[]> {
  const { invoke } = await import('@tauri-apps/api/core')
  return invoke('call_recordings')
}
