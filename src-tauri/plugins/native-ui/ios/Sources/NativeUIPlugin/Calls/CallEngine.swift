import AVFoundation
import Foundation
import WebRTC

/// TURN details the engine hands out for one call (calls/mod.rs turn_info).
struct CallTurn: Decodable, Equatable {
    let urls: [String]
    let username: String
    let credential: String
}

/// One call's WebRTC side on the iPhone (docs/CALLS.md), set up exactly like the
/// desktop call screen (src/lib/calls.ts): its ONLY ICE server is DropBeam's own
/// TURN bridge on 127.0.0.1 (relay policy), so every packet goes through the
/// engine and the friend's iroh connection; DTLS-SRTP end to end; audio and video
/// transceivers always (the camera can come on later without renegotiating);
/// non-trickle (the bridge answers allocations at once); Opus up to 128 kbps,
/// H.264 (hardware) preferred, 720p30 at up to 2.5 Mbps.
final class CallEngine: NSObject, RTCPeerConnectionDelegate {
    static let factory: RTCPeerConnectionFactory = {
        RTCInitializeSSL()
        // Our own audio device (the same voice-processing unit WebRTC's would use),
        // so a recording can hear both voices (CallAudioDevice).
        return RTCPeerConnectionFactory(encoderFactory: RTCDefaultVideoEncoderFactory(), decoderFactory: RTCDefaultVideoDecoderFactory(), audioDevice: CallAudioDevice.shared)
    }()

    private(set) var pc: RTCPeerConnection?
    private var audioTrack: RTCAudioTrack?
    private(set) var localVideo: RTCVideoTrack?
    private var videoSource: RTCVideoSource?
    private var capturer: RTCCameraVideoCapturer?
    private(set) var remoteVideo: RTCVideoTrack?
    private(set) var front = true
    private(set) var cameraOn = false
    private var gathering: [CheckedContinuation<Void, Never>] = []
    /// Called on the main queue.
    var onConnection: ((RTCPeerConnectionState) -> Void)?
    var onRemoteVideo: ((RTCVideoTrack?) -> Void)?

    init(turn: CallTurn) {
        super.init()
        let config = RTCConfiguration()
        config.iceServers = [RTCIceServer(urlStrings: turn.urls, username: turn.username, credential: turn.credential)]
        // Every packet goes through DropBeam's bridge — never straight onto the network.
        config.iceTransportPolicy = .relay
        config.bundlePolicy = .maxBundle
        config.rtcpMuxPolicy = .require
        config.sdpSemantics = .unifiedPlan
        config.continualGatheringPolicy = .gatherOnce
        let constraints = RTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil)
        pc = Self.factory.peerConnection(with: config, constraints: constraints, delegate: self)
    }

    // ── media ──

    private func makeTracks() {
        guard audioTrack == nil else { return }
        // Echo cancellation / noise suppression come from the iOS voice-processing I/O unit.
        let audio = Self.factory.audioSource(with: RTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil))
        audioTrack = Self.factory.audioTrack(with: audio, trackId: "a0")
        let source = Self.factory.videoSource()
        videoSource = source
        localVideo = Self.factory.videoTrack(with: source, trackId: "v0")
        capturer = RTCCameraVideoCapturer(delegate: source)
    }

    /// Caller: both transceivers, then the offer.
    func addMedia(video: Bool) {
        makeTracks()
        guard let pc, let audioTrack, let localVideo else { return }
        let sendRecv = RTCRtpTransceiverInit()
        sendRecv.direction = .sendRecv
        sendRecv.streamIds = ["s0"]
        pc.addTransceiver(with: audioTrack, init: sendRecv)
        let v = pc.addTransceiver(with: localVideo, init: sendRecv)
        if !video { v?.sender.track = nil }
        preferH264()
        tune()
        if video { setCamera(true) }
    }

    /// Callee: the offer made the transceivers; attach our tracks and send both ways.
    func attachToOffer(video: Bool) {
        makeTracks()
        guard let pc else { return }
        for t in pc.transceivers {
            var error: NSError?
            t.setDirection(.sendRecv, error: &error)
            switch t.mediaType {
            case .audio: t.sender.track = audioTrack
            case .video: t.sender.track = video ? localVideo : nil
            default: break
            }
        }
        preferH264()
        tune()
        if video { setCamera(true) }
    }

    /// H.264 first (VideoToolbox hardware encoder on iPhone), then the rest.
    private func preferH264() {
        guard let pc else { return }
        let caps = Self.factory.rtpReceiverCapabilities(forKind: kRTCMediaStreamTrackKindVideo).codecs
        let h264 = caps.filter { $0.name.caseInsensitiveCompare(kRTCVideoCodecH264Name) == .orderedSame }
        guard !h264.isEmpty else { return }
        let rest = caps.filter { $0.name.caseInsensitiveCompare(kRTCVideoCodecH264Name) != .orderedSame }
        for t in pc.transceivers where t.mediaType == .video {
            try? t.setCodecPreferences(h264 + rest)
        }
    }

    /// Opus up to 128 kbps (high network priority); video up to 2.5 Mbps at 30 fps.
    private func tune() {
        guard let pc else { return }
        for t in pc.transceivers {
            let params = t.sender.parameters
            guard let enc = params.encodings.first else { continue }
            if t.mediaType == .audio {
                enc.maxBitrateBps = NSNumber(value: 128_000)
                enc.networkPriority = .high
            } else {
                enc.maxBitrateBps = NSNumber(value: 2_500_000)
                enc.maxFramerate = NSNumber(value: 30)
            }
            t.sender.parameters = params
        }
    }

    func setMuted(_ muted: Bool) { audioTrack?.isEnabled = !muted }

    /// Camera on/off without renegotiating: the video sender gets or drops the track.
    func setCamera(_ on: Bool) {
        guard let pc, let localVideo else { return }
        let sender = pc.transceivers.first { $0.mediaType == .video }?.sender
        if on {
            startCapture()
            sender?.track = localVideo
        } else {
            capturer?.stopCapture()
            sender?.track = nil
        }
        cameraOn = on
    }

    func switchCamera() {
        front.toggle()
        if cameraOn { startCapture() }
    }

    private func startCapture() {
        guard let capturer else { return }
        let position: AVCaptureDevice.Position = front ? .front : .back
        guard let device = RTCCameraVideoCapturer.captureDevices().first(where: { $0.position == position })
                ?? RTCCameraVideoCapturer.captureDevices().first else { return }
        // The format closest to 1280×720 that can do 30 fps.
        let formats = RTCCameraVideoCapturer.supportedFormats(for: device).filter { f in
            f.videoSupportedFrameRateRanges.contains { $0.maxFrameRate >= 30 }
        }
        let best = formats.min { a, b in
            let da = CMVideoFormatDescriptionGetDimensions(a.formatDescription), db = CMVideoFormatDescriptionGetDimensions(b.formatDescription)
            return abs(Int(da.width) - 1280) + abs(Int(da.height) - 720) < abs(Int(db.width) - 1280) + abs(Int(db.height) - 720)
        }
        guard let format = best ?? RTCCameraVideoCapturer.supportedFormats(for: device).last else { return }
        capturer.startCapture(with: device, format: format, fps: 30)
    }

    // ── offer / answer ──

    private func waitGathered() async {
        guard let pc, pc.iceGatheringState != .complete else { return }
        await withCheckedContinuation { (c: CheckedContinuation<Void, Never>) in
            DispatchQueue.main.async {
                if pc.iceGatheringState == .complete { c.resume(); return }
                self.gathering.append(c)
                // Every candidate comes from the local bridge; never wait on a dead interface.
                DispatchQueue.main.asyncAfter(deadline: .now() + 3) { self.finishGathering() }
            }
        }
    }

    private func finishGathering() {
        let waiting = gathering
        gathering = []
        for c in waiting { c.resume() }
    }

    private static let mediaConstraints = RTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil)

    func makeOffer() async throws -> String {
        guard let pc else { throw CallError.closed }
        let offer: RTCSessionDescription = try await withCheckedThrowingContinuation { c in
            pc.offer(for: Self.mediaConstraints) { sdp, error in
                if let sdp { c.resume(returning: sdp) } else { c.resume(throwing: error ?? CallError.closed) }
            }
        }
        try await setLocal(offer)
        await waitGathered()
        guard let local = pc.localDescription?.sdp else { throw CallError.closed }
        return local
    }

    /// Callee, after `applyRemote(.offer)` + `attachToOffer`: the answer.
    func makeAnswerForAppliedOffer() async throws -> String {
        guard let pc else { throw CallError.closed }
        let answer: RTCSessionDescription = try await withCheckedThrowingContinuation { c in
            pc.answer(for: Self.mediaConstraints) { sdp, error in
                if let sdp { c.resume(returning: sdp) } else { c.resume(throwing: error ?? CallError.closed) }
            }
        }
        try await setLocal(answer)
        await waitGathered()
        guard let local = pc.localDescription?.sdp else { throw CallError.closed }
        return local
    }

    /// The callee's answer (or a renegotiation from the other side).
    func applyRemote(type: RTCSdpType, sdp: String) async throws {
        try await setRemote(RTCSessionDescription(type: type, sdp: sdp))
    }

    private func setLocal(_ d: RTCSessionDescription) async throws {
        guard let pc else { throw CallError.closed }
        try await withCheckedThrowingContinuation { (c: CheckedContinuation<Void, Error>) in
            pc.setLocalDescription(d) { error in if let error { c.resume(throwing: error) } else { c.resume() } }
        }
    }

    private func setRemote(_ d: RTCSessionDescription) async throws {
        guard let pc else { throw CallError.closed }
        try await withCheckedThrowingContinuation { (c: CheckedContinuation<Void, Error>) in
            pc.setRemoteDescription(d) { error in if let error { c.resume(throwing: error) } else { c.resume() } }
        }
    }

    // ── stats ──

    struct Sample { var rttMs: Double?; var bytesSent: Double = 0; var bytesReceived: Double = 0; var lost: Double = 0; var received: Double = 0
        var jitterMs: Double?; var width: Double?; var height: Double?; var fps: Double? }

    func sample() async -> Sample {
        guard let pc else { return Sample() }
        let report: RTCStatisticsReport = await withCheckedContinuation { c in pc.statistics { c.resume(returning: $0) } }
        var s = Sample()
        func num(_ v: NSObject?) -> Double? { (v as? NSNumber)?.doubleValue }
        for stat in report.statistics.values {
            let v = stat.values
            switch stat.type {
            case "candidate-pair" where (v["state"] as? String) == "succeeded" && ((v["nominated"] as? NSNumber)?.boolValue ?? false):
                if let rtt = num(v["currentRoundTripTime"]) { s.rttMs = rtt * 1000 }
            case "outbound-rtp": s.bytesSent += num(v["bytesSent"]) ?? 0
            case "inbound-rtp":
                s.bytesReceived += num(v["bytesReceived"]) ?? 0
                if (v["kind"] as? String) == "audio" {
                    s.lost += num(v["packetsLost"]) ?? 0
                    s.received += num(v["packetsReceived"]) ?? 0
                    if let j = num(v["jitter"]) { s.jitterMs = j * 1000 }
                } else if let w = num(v["frameWidth"]) {
                    s.width = w; s.height = num(v["frameHeight"]); s.fps = num(v["framesPerSecond"])
                }
            default: break
            }
        }
        return s
    }

    func close() {
        capturer?.stopCapture()
        pc?.close()
        pc = nil
        finishGathering()
    }

    // ── RTCPeerConnectionDelegate (WebRTC's signaling thread) ──

    func peerConnection(_ peerConnection: RTCPeerConnection, didChange stateChanged: RTCSignalingState) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didAdd stream: RTCMediaStream) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didRemove stream: RTCMediaStream) {}
    func peerConnectionShouldNegotiate(_ peerConnection: RTCPeerConnection) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceConnectionState) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didGenerate candidate: RTCIceCandidate) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didRemove candidates: [RTCIceCandidate]) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didOpen dataChannel: RTCDataChannel) {}
    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceGatheringState) {
        guard newState == .complete else { return }
        DispatchQueue.main.async { self.finishGathering() }
    }
    func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCPeerConnectionState) {
        DispatchQueue.main.async { self.onConnection?(newState) }
    }
    func peerConnection(_ peerConnection: RTCPeerConnection, didStartReceivingOn transceiver: RTCRtpTransceiver) {
        guard transceiver.mediaType == .video, let track = transceiver.receiver.track as? RTCVideoTrack else { return }
        DispatchQueue.main.async {
            self.remoteVideo = track
            self.onRemoteVideo?(track)
        }
    }
}

enum CallError: LocalizedError {
    case closed, busy, microphone
    var errorDescription: String? {
        switch self {
        case .closed: return "The call has ended."
        case .busy: return "You’re already in a call."
        case .microphone: return "DropBeam needs the microphone to call. Allow it in Settings → DropBeam."
        }
    }
}
