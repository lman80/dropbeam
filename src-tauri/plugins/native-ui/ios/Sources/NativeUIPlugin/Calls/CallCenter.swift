import AVFoundation
import CallKit
import CryptoKit
import Foundation
import PushKit
import UIKit
import WebRTC

/// The engine's call snapshot (calls/mod.rs Calls::snapshot).
struct CallSnapshot: Decodable, Equatable, Identifiable {
    struct Leg: Decodable, Equatable { let endpoint: String; let state: String }
    struct Recording: Decodable, Equatable {
        var askedByMe = false, theyAllow = false, askedByThem = false, iAllow = false
        var meRecording = false, themRecording = false, iShare = false, theyShare = false
    }
    let id: String
    let dir: String
    let person: String
    let name: String
    let video: Bool
    let phase: String
    let startedMs: Double
    let answeredMs: Double?
    let mediaMs: Double?
    let endedMs: Double?
    let end: String?
    let legs: [Leg]
    let active: String?
    var autoAccept: Bool?
    var glare: Bool?
    var recording: Recording?
    var incoming: Bool { dir == "in" }
    var ringing: Bool { phase == "ringing" }
}

private struct StartResult: Decodable { let callId: String; let turn: CallTurn; let call: CallSnapshot }
private struct AcceptResult: Decodable { let callId: String; let turn: CallTurn; let sdp: String; let video: Bool; let name: String }
private struct EngineStats: Decodable {
    struct Path: Decodable { let path: String?; let rttMs: Double? }
    let path: Path?
}

struct CallQuality: Equatable {
    var path: String?
    var rttMs: Int?
    var lossPct: Double?
    var jitterMs: Int?
    var sendKbps: Int?
    var recvKbps: Int?
    var video: String?
    /// good | ok | poor
    var grade: String?
}

/// Calls on the iPhone (docs/CALLS.md): CallKit for ringing (also on the lock
/// screen) and the system call UI, PushKit to be woken for a call, and the
/// WebRTC engine (CallEngine) under the same Rust commands the desktop call
/// screen uses. The audio session follows CallKit: WebRTC's audio unit only
/// runs between provider didActivate/didDeactivate (manual audio).
@MainActor
final class CallCenter: NSObject, ObservableObject {
    static let shared = CallCenter()

    @Published private(set) var call: CallSnapshot?
    /// The call that just ended (the "Call ended" card).
    @Published var last: CallSnapshot?
    @Published private(set) var muted = false
    @Published private(set) var cameraOn = false
    @Published private(set) var remoteMuted = false
    @Published private(set) var remoteCamera = false
    @Published private(set) var remoteVideo: RTCVideoTrack?
    @Published private(set) var localVideo: RTCVideoTrack?
    @Published private(set) var quality: CallQuality?
    @Published private(set) var busy = false
    @Published private(set) var speaker = false
    /// CallKit couldn't show the ring (simulator, or a CallKit refusal): ring in the app.
    @Published private(set) var ringInApp = false

    private var engine: CallEngine?
    private var callId: String?
    private var uuids: [String: UUID] = [:]
    /// Calls CallKit was told about from a VoIP push, before the engine knew them.
    private var pushed: [String: UUID] = [:]
    private var pendingAnswer: String?
    private var statsTimer: Timer?
    private var lastSample: (at: Date, sent: Double, recv: Double, lost: Double, got: Double)?
    private var observers: [NSObjectProtocol] = []
    private var started = false
    private var voipToken: (String, String)?

    private let provider: CXProvider = {
        let config = CXProviderConfiguration()
        config.supportsVideo = true
        config.maximumCallGroups = 1
        config.maximumCallsPerCallGroup = 1
        config.supportedHandleTypes = [.generic]
        config.includesCallsInRecents = false
        return CXProvider(configuration: config)
    }()
    private let controller = CXCallController()
    private var registry: PKPushRegistry?

    private override init() { super.init() }

    private var bridge: Bridge { Bridge.shared }

    /// As early as possible (plugin load): PushKit delivers a waiting VoIP push
    /// only once a registry exists, and it must be reported to CallKit at once.
    func registerPushKit() {
        guard registry == nil else { return }
        let r = PKPushRegistry(queue: .main)
        r.delegate = self
        r.desiredPushTypes = [.voIP]
        registry = r
        provider.setDelegate(self, queue: .main)
        RTCAudioSession.sharedInstance().useManualAudio = true
        RTCAudioSession.sharedInstance().isAudioEnabled = false
    }

    /// Once the bridge is up (plugin activate): follow the engine's call events
    /// and tell it this iPhone can take calls.
    func start() {
        registerPushKit()
        guard !started else { return }
        started = true
        for name in ["call://state", "call://incoming", "call://signal", "call://dial"] {
            observers.append(NotificationCenter.default.addObserver(forName: Notification.Name("DropBeam.\(name)"), object: nil, queue: .main) { note in
                let payload = note.userInfo?["payload"]
                MainActor.assumeIsolated { CallCenter.shared.onEvent(name, payload) }
            })
        }
        Task {
            try? await bridge.action("callCapable", ["supported": true])
            if let (token, env) = voipToken { try? await bridge.action("pushSetVoipToken", ["token": token, "env": env]) }
            // A call that was already ringing (woken by a push, engine got there first).
            if let status: StatusResult = try? await bridge.call("callStatus"), let c = status.call { onState(c) ; if c.incoming && c.ringing { onIncoming(c) } }
        }
    }
    private struct StatusResult: Decodable { let call: CallSnapshot? }

    // ── engine events ──

    private func decode<T: Decodable>(_ payload: Any?) -> T? {
        guard let payload, let data = try? JSONSerialization.data(withJSONObject: payload) else { return nil }
        return try? JSONDecoder().decode(T.self, from: data)
    }

    private func onEvent(_ name: String, _ payload: Any?) {
        switch name {
        case "call://incoming": if let c: CallSnapshot = decode(payload) { onIncoming(c) }
        case "call://state": if let c: CallSnapshot = decode(payload) { onState(c) }
        case "call://signal": onSignal(payload as? [String: Any] ?? [:])
        case "call://dial":
            let p = payload as? [String: Any] ?? [:]
            if let friend = p["friendId"] as? String { startCall(friendId: friend, video: p["video"] as? Bool ?? false) }
        default: break
        }
    }

    private func onIncoming(_ c: CallSnapshot) {
        guard call == nil || call?.id == c.id else { return }
        call = c
        last = nil
        // Declined from the lock screen before the engine had the call.
        if declinedEarly.remove(c.id) != nil { Task { try? await bridge.action("callDecline", ["callId": c.id]) }; return }
        if let uuid = pushed.removeValue(forKey: c.id) {
            // Already ringing from the push: name it properly.
            uuids[c.id] = uuid
            provider.reportCall(with: uuid, updated: update(for: c))
            if answerEarly.remove(c.id) != nil { Task { await answer() } }
            return
        }
        let uuid = UUID()
        uuids[c.id] = uuid
        let auto = c.autoAccept == true
        provider.reportNewIncomingCall(with: uuid, update: update(for: c)) { error in
            Task { @MainActor in
                let center = CallCenter.shared
                if error != nil {
                    // No CallKit here (simulator): ring in the app, or answer at once.
                    center.uuids.removeValue(forKey: c.id)
                    if auto { center.answerInApp() } else { center.ringInApp = true }
                } else if auto {
                    // Both called each other at once: this call connects without a ring.
                    center.request(CXAnswerCallAction(call: uuid))
                }
            }
        }
    }
    /// Answered / declined on the lock screen while only the VoIP push had arrived.
    private var answerEarly: Set<String> = []
    private var declinedEarly: Set<String> = []

    private func update(for c: CallSnapshot) -> CXCallUpdate {
        let u = CXCallUpdate()
        u.remoteHandle = CXHandle(type: .generic, value: c.person)
        u.localizedCallerName = c.name
        u.hasVideo = c.video
        u.supportsHolding = false
        u.supportsGrouping = false
        u.supportsUngrouping = false
        u.supportsDTMF = false
        return u
    }

    private func onState(_ c: CallSnapshot) {
        if c.phase == "ended" {
            guard callId == nil || callId == c.id || call?.id == c.id else { return }
            if let uuid = uuids.removeValue(forKey: c.id) {
                let reason: CXCallEndedReason
                switch c.end {
                case "remote_hangup": reason = .remoteEnded
                case "answered_elsewhere": reason = .answeredElsewhere
                case "missed", "no_answer", "canceled": reason = .unanswered
                case "declined": reason = c.incoming ? .declinedElsewhere : .remoteEnded
                case "hangup": reason = .remoteEnded
                default: reason = .failed
                }
                // Our own hang-up already went through a CXEndCallAction; reporting again is harmless.
                provider.reportCall(with: uuid, endedAt: Date(), reason: reason)
            }
            teardown()
            ringInApp = false
            call = nil
            if c.glare != true && c.end != "answered_elsewhere" { last = c }
            return
        }
        if let current = call, current.id != c.id, current.phase != "ended" { return }
        call = c
        if c.phase != "ringing" { ringInApp = false }
        if !c.incoming, let uuid = uuids[c.id] {
            if c.phase == "connecting" { provider.reportOutgoingCall(with: uuid, startedConnectingAt: Date()) }
            if c.phase == "active" { provider.reportOutgoingCall(with: uuid, connectedAt: Date()) }
        }
    }

    private func onSignal(_ s: [String: Any]) {
        guard let kind = s["kind"] as? String else { return }
        if kind == "call.answer", let sdp = s["sdp"] as? String {
            guard (s["callId"] as? String) == callId else { return }
            guard let engine else { pendingAnswer = sdp; return }
            Task { do { try await engine.applyRemote(type: .answer, sdp: sdp) } catch { bridge.errorMessage = error.localizedDescription } }
            return
        }
        guard (s["callId"] as? String) == callId, let body = s["body"] as? [String: Any] else { return }
        if kind == "call.media" {
            remoteMuted = body["muted"] as? Bool ?? false
            remoteCamera = body["camera"] as? Bool ?? false
        } else if kind == "call.record", body["action"] as? String == "deny" {
            bridge.showToast("\(call?.name ?? "They") didn’t allow recording.")
        }
    }

    // ── placing and answering ──

    func startCall(friendId: String, video: Bool) {
        guard call == nil else { bridge.errorMessage = CallError.busy.localizedDescription; return }
        busy = true
        Task {
            defer { busy = false }
            do {
                try await ensurePermissions(video: video)
                let started: StartResult = try await bridge.call("callStart", ["friendId": friendId, "video": video])
                callId = started.callId
                call = started.call
                last = nil
                let uuid = UUID()
                uuids[started.callId] = uuid
                // provider(perform: CXStartCallAction) places the call with these.
                pendingTurn = (started.turn, video)
                let handle = CXHandle(type: .generic, value: started.call.person)
                let action = CXStartCallAction(call: uuid, handle: handle)
                action.isVideo = video
                action.contactIdentifier = started.call.name
                do {
                    try await controller.request(CXTransaction(action: action))
                    // provider(perform: CXStartCallAction) runs placeCall.
                } catch {
                    // No CallKit (simulator): place it ourselves.
                    uuids.removeValue(forKey: started.callId)
                    activateAudioWithoutCallKit()
                    try await placeCall(turn: started.turn, video: video)
                }
            } catch {
                if let id = callId { try? await bridge.action("callHangup", ["callId": id]) }
                teardown()
                call = nil
                bridge.errorMessage = error.localizedDescription
            }
        }
    }
    private var pendingTurn: (CallTurn, Bool)?
    private var placing = false

    private func placeCall(turn: CallTurn, video: Bool) async throws {
        guard let id = callId, engine == nil, !placing else { return }
        placing = true
        defer { placing = false }
        let e = makeEngine(turn: turn)
        e.addMedia(video: video)
        cameraOn = video
        localVideo = e.localVideo
        let offer = try await e.makeOffer()
        try await bridge.action("callOffer", ["callId": id, "sdp": offer])
        if let answer = pendingAnswer { pendingAnswer = nil; try await e.applyRemote(type: .answer, sdp: answer) }
        startStats()
    }

    private func makeEngine(turn: CallTurn) -> CallEngine {
        let e = CallEngine(turn: turn)
        e.onConnection = { [weak self] state in
            guard let self, let id = self.callId else { return }
            if state == .connected { Task { try? await self.bridge.action("callMediaUp", ["callId": id]) } }
            if state == .failed { self.hangup() }
        }
        e.onRemoteVideo = { [weak self] track in self?.remoteVideo = track }
        engine = e
        return e
    }

    /// The person answered (CallKit or the in-app card).
    func answer() async {
        guard let c = call, c.incoming, c.ringing, engine == nil else { return }
        busy = true
        defer { busy = false }
        do {
            try await ensurePermissions(video: c.video)
            let acc: AcceptResult = try await bridge.call("callAccept", ["callId": c.id])
            callId = c.id
            let e = makeEngine(turn: acc.turn)
            // The offer creates the transceivers; then our tracks join them.
            try await e.applyRemote(type: .offer, sdp: acc.sdp)
            e.attachToOffer(video: acc.video)
            cameraOn = acc.video
            localVideo = e.localVideo
            let answer = try await e.makeAnswerForAppliedOffer()
            try await bridge.action("callAnswer", ["callId": c.id, "sdp": answer])
            startStats()
        } catch {
            try? await bridge.action("callDecline", ["callId": c.id])
            teardown()
            bridge.errorMessage = error.localizedDescription
        }
    }

    func decline() {
        guard let c = call else { return }
        if let uuid = uuids[c.id] { request(CXEndCallAction(call: uuid)) } else { Task { try? await bridge.action("callDecline", ["callId": c.id]) } }
    }

    func hangup() {
        guard let c = call else { return }
        if let uuid = uuids[c.id] { request(CXEndCallAction(call: uuid)) } else { endInEngine(c) }
    }

    private func endInEngine(_ c: CallSnapshot) {
        Task {
            if c.incoming && c.ringing { try? await bridge.action("callDecline", ["callId": c.id]) }
            else { try? await bridge.action("callHangup", ["callId": c.id]) }
        }
    }

    private func request(_ action: CXAction) {
        controller.request(CXTransaction(action: action)) { error in
            guard error != nil else { return }
            // CallKit unavailable: do it directly.
            Task { @MainActor in
                let center = CallCenter.shared
                if let c = center.call {
                    if action is CXEndCallAction { center.endInEngine(c) }
                    if let mute = action as? CXSetMutedCallAction { center.applyMute(mute.isMuted) }
                }
            }
        }
    }

    // ── in-call controls ──

    func toggleMute() {
        guard let c = call else { return }
        if let uuid = uuids[c.id] { request(CXSetMutedCallAction(call: uuid, muted: !muted)) } else { applyMute(!muted) }
    }

    private func applyMute(_ on: Bool) {
        muted = on
        engine?.setMuted(on)
        signalMedia()
    }

    func toggleCamera() {
        guard let engine else { return }
        if !cameraOn {
            Task {
                guard await AVCaptureDevice.requestAccess(for: .video) else { bridge.errorMessage = "Allow the camera for DropBeam in Settings to use video."; return }
                engine.setCamera(true)
                cameraOn = true
                signalMedia()
            }
        } else {
            engine.setCamera(false)
            cameraOn = false
            signalMedia()
        }
    }

    func switchCamera() { engine?.switchCamera() }

    func toggleSpeaker() {
        speaker.toggle()
        let session = RTCAudioSession.sharedInstance()
        session.lockForConfiguration()
        try? session.overrideOutputAudioPort(speaker ? .speaker : .none)
        session.unlockForConfiguration()
    }

    private func signalMedia() {
        guard let id = callId else { return }
        Task { try? await bridge.action("callSignal", ["callId": id, "frame": ["kind": "call.media", "muted": muted, "camera": cameraOn]]) }
    }

    /// Their request to record this call: yes or no (the engine enforces it).
    func answerRecordRequest(_ allow: Bool) {
        guard let id = callId else { return }
        Task { try? await bridge.action("callSignal", ["callId": id, "frame": ["kind": "call.record", "action": allow ? "allow" : "deny"]]) }
    }

    // ── quality ──

    private func startStats() {
        statsTimer?.invalidate()
        statsTimer = Timer.scheduledTimer(withTimeInterval: 2, repeats: true) { _ in
            Task { @MainActor in await CallCenter.shared.sampleStats() }
        }
    }

    private func sampleStats() async {
        guard let engine, let id = callId else { return }
        let s = await engine.sample()
        let now = Date()
        var q = CallQuality()
        if let last = lastSample {
            let secs = max(0.5, now.timeIntervalSince(last.at))
            q.sendKbps = Int((s.bytesSent - last.sent) * 8 / 1000 / secs)
            q.recvKbps = Int((s.bytesReceived - last.recv) * 8 / 1000 / secs)
            let dl = s.lost - last.lost, dg = s.received - last.got
            q.lossPct = dl + dg > 0 ? max(0, (1000 * dl / (dl + dg)).rounded() / 10) : 0
        }
        lastSample = (now, s.bytesSent, s.bytesReceived, s.lost, s.received)
        let stats: EngineStats? = try? await bridge.call("callStats", ["callId": id])
        q.path = stats?.path?.path
        let rtt = s.rttMs ?? stats?.path?.rttMs
        q.rttMs = rtt.map { Int($0) }
        q.jitterMs = s.jitterMs.map { Int($0) }
        if let w = s.width, let h = s.height { q.video = "\(Int(w))×\(Int(h)) \(Int(s.fps ?? 0)) fps" }
        if let r = rtt { q.grade = r > 400 || (q.lossPct ?? 0) > 8 ? "poor" : r > 200 || (q.lossPct ?? 0) > 3 ? "ok" : "good" }
        quality = q
    }

    // ── teardown / permissions / audio ──

    private func teardown() {
        statsTimer?.invalidate()
        statsTimer = nil
        lastSample = nil
        engine?.close()
        engine = nil
        callId = nil
        pendingAnswer = nil
        pendingTurn = nil
        remoteVideo = nil
        localVideo = nil
        muted = false
        cameraOn = false
        remoteMuted = false
        remoteCamera = false
        quality = nil
        if speaker { toggleSpeaker() }
        if !uuidsInUse { deactivateAudioWithoutCallKit() }
    }
    private var uuidsInUse: Bool { !uuids.isEmpty }

    private func ensurePermissions(video: Bool) async throws {
        guard await AVAudioApplication.requestRecordPermission() else { throw CallError.microphone }
        if video { _ = await AVCaptureDevice.requestAccess(for: .video) }
    }

    private func configureAudio(video: Bool) {
        let config = RTCAudioSessionConfiguration.webRTC()
        config.category = AVAudioSession.Category.playAndRecord.rawValue
        config.mode = (video ? AVAudioSession.Mode.videoChat : AVAudioSession.Mode.voiceChat).rawValue
        config.categoryOptions = [.allowBluetoothHFP, .allowBluetoothA2DP, .allowAirPlay]
        let session = RTCAudioSession.sharedInstance()
        session.lockForConfiguration()
        try? session.setConfiguration(config)
        session.unlockForConfiguration()
    }

    /// Simulator / no CallKit: run the audio session ourselves.
    private var manualAudio = false
    private func activateAudioWithoutCallKit() {
        manualAudio = true
        configureAudio(video: call?.video ?? false)
        let session = RTCAudioSession.sharedInstance()
        session.lockForConfiguration()
        try? session.setActive(true)
        session.unlockForConfiguration()
        session.isAudioEnabled = true
    }
    private func deactivateAudioWithoutCallKit() {
        guard manualAudio else { return }
        manualAudio = false
        let session = RTCAudioSession.sharedInstance()
        session.isAudioEnabled = false
        session.lockForConfiguration()
        try? session.setActive(false)
        session.unlockForConfiguration()
    }

    /// In-app Answer (no CallKit).
    func answerInApp() {
        ringInApp = false
        activateAudioWithoutCallKit()
        Task { await answer() }
    }
}

// ── CallKit ──

extension CallCenter: CXProviderDelegate {
    nonisolated func providerDidReset(_ provider: CXProvider) {
        MainActor.assumeIsolated {
            if let c = call { endInEngine(c) }
            uuids.removeAll()
            teardown()
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXStartCallAction) {
        MainActor.assumeIsolated {
            guard let (turn, video) = pendingTurn else { action.fail(); return }
            configureAudio(video: video)
            Task {
                do { try await placeCall(turn: turn, video: video); action.fulfill() }
                catch { action.fail(); hangup() }
            }
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXAnswerCallAction) {
        MainActor.assumeIsolated {
            configureAudio(video: call?.video ?? false)
            if call == nil, let id = pushed.first(where: { $0.value == action.callUUID })?.key {
                // The engine hasn't got the call yet: answer it the moment it does.
                answerEarly.insert(id)
                action.fulfill()
                return
            }
            Task { await answer(); action.fulfill() }
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXEndCallAction) {
        MainActor.assumeIsolated {
            if let c = call, uuids[c.id] == action.callUUID { endInEngine(c) }
            // A push-reported call the engine hasn't learned about yet: decline it when it does.
            if let id = pushed.first(where: { $0.value == action.callUUID })?.key {
                pushed.removeValue(forKey: id)
                declinedEarly.insert(id)
            }
            action.fulfill()
        }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXSetMutedCallAction) {
        MainActor.assumeIsolated { applyMute(action.isMuted); action.fulfill() }
    }

    nonisolated func provider(_ provider: CXProvider, perform action: CXSetHeldCallAction) {
        // DropBeam calls don't hold; treat it as mute.
        MainActor.assumeIsolated { applyMute(action.isOnHold); action.fulfill() }
    }

    nonisolated func provider(_ provider: CXProvider, didActivate audioSession: AVAudioSession) {
        let rtc = RTCAudioSession.sharedInstance()
        rtc.audioSessionDidActivate(audioSession)
        rtc.isAudioEnabled = true
    }

    nonisolated func provider(_ provider: CXProvider, didDeactivate audioSession: AVAudioSession) {
        let rtc = RTCAudioSession.sharedInstance()
        rtc.isAudioEnabled = false
        rtc.audioSessionDidDeactivate(audioSession)
    }
}

// ── PushKit (VoIP wake-up, calls/wake.rs) ──

extension CallCenter: PKPushRegistryDelegate {
    nonisolated func pushRegistry(_ registry: PKPushRegistry, didUpdate pushCredentials: PKPushCredentials, for type: PKPushType) {
        let token = pushCredentials.token.map { String(format: "%02x", $0) }.joined()
        #if DEBUG
        let env = "sandbox"
        #else
        let env = "prod"
        #endif
        MainActor.assumeIsolated {
            voipToken = (token, env)
            if started { Task { try? await bridge.action("pushSetVoipToken", ["token": token, "env": env]) } }
        }
    }

    nonisolated func pushRegistry(_ registry: PKPushRegistry, didInvalidatePushTokenFor type: PKPushType) {
        MainActor.assumeIsolated {
            voipToken = ("", "prod")
            if started { Task { try? await bridge.action("pushSetVoipToken", ["token": "", "env": "prod"]) } }
        }
    }

    /// iOS requires every VoIP push to be reported to CallKit before this returns.
    nonisolated func pushRegistry(_ registry: PKPushRegistry, didReceiveIncomingPushWith payload: PKPushPayload, for type: PKPushType, completion: @escaping () -> Void) {
        let dict = payload.dictionaryPayload
        MainActor.assumeIsolated {
            let id = dict["c"] as? String ?? UUID().uuidString
            let video = dict["v"] as? Bool ?? false
            let from = (dict["s"] as? String).flatMap(Self.openSealed)?["f"] as? String ?? dict["f"] as? String
            let names = Self.knownNames()
            let name = from.flatMap { names[$0] }
            // Already ringing for this call (the engine got there first)? Nothing to add.
            if call?.id == id || pushed[id] != nil {
                completion()
                return
            }
            let uuid = UUID()
            pushed[id] = uuid
            let u = CXCallUpdate()
            u.remoteHandle = CXHandle(type: .generic, value: from ?? "DropBeam")
            u.localizedCallerName = name ?? "DropBeam"
            u.hasVideo = video
            provider.reportNewIncomingCall(with: uuid, update: u) { _ in completion() }
            if name == nil {
                // Not one of this phone's friends or devices: never ring for it.
                provider.reportCall(with: uuid, endedAt: Date(), reason: .failed)
                pushed.removeValue(forKey: id)
                return
            }
            // The caller redials every 2 s; if its call doesn't reach us soon, stop ringing.
            DispatchQueue.main.asyncAfter(deadline: .now() + 20) {
                let center = CallCenter.shared
                if let still = center.pushed.removeValue(forKey: id), center.call?.id != id {
                    center.provider.reportCall(with: still, endedAt: Date(), reason: .failed)
                }
            }
        }
    }

    /// endpoint id → friend name (PushRegistration.saveNames), from YOUR contacts.
    private static func knownNames() -> [String: String] {
        guard let url = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: PushRegistration.appGroup)?.appendingPathComponent("push-names.json"),
              let data = try? Data(contentsOf: url),
              let map = try? JSONSerialization.jsonObject(with: data) as? [String: String] else { return [:] }
        return map
    }

    /// Open `mailbox::seal::seal_small` (same as DropBeamNotify).
    private static func openSealed(_ sealed: String) -> [String: Any]? {
        guard let raw = Data(base64Encoded: sealed), raw.count > 32 + 12 + 16,
              let me = PushRegistration.pushKey(),
              let eph = try? Curve25519.KeyAgreement.PublicKey(rawRepresentation: raw.prefix(32)),
              let shared = try? me.sharedSecretFromKeyAgreement(with: eph) else { return nil }
        let salt = raw.prefix(32) + me.publicKey.rawRepresentation
        let key = shared.hkdfDerivedSymmetricKey(using: SHA256.self, salt: salt, sharedInfo: Data("dropbeam-push-v1".utf8), outputByteCount: 32)
        guard let box = try? ChaChaPoly.SealedBox(combined: raw.dropFirst(32)),
              let plain = try? ChaChaPoly.open(box, using: key) else { return nil }
        return try? JSONSerialization.jsonObject(with: plain) as? [String: Any]
    }
}
