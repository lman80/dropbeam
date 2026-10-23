import AVKit
import SwiftUI
import WebRTC

/// A WebRTC video track on screen (Metal).
struct RTCVideo: UIViewRepresentable {
    let track: RTCVideoTrack?
    var fill = true
    final class Holder { var track: RTCVideoTrack? }
    func makeCoordinator() -> Holder { Holder() }
    func makeUIView(context: Context) -> RTCMTLVideoView {
        let v = RTCMTLVideoView(frame: .zero)
        v.videoContentMode = fill ? .scaleAspectFill : .scaleAspectFit
        v.backgroundColor = .black
        return v
    }
    func updateUIView(_ view: RTCMTLVideoView, context: Context) {
        guard context.coordinator.track !== track else { return }
        context.coordinator.track?.remove(view)
        track?.add(view)
        context.coordinator.track = track
    }
    static func dismantleUIView(_ view: RTCMTLVideoView, coordinator: Holder) {
        coordinator.track?.remove(view)
    }
}

/// Over everything: the in-app ring (when CallKit can't), the call, "Call ended".
struct CallOverlay: View {
    @ObservedObject private var calls = CallCenter.shared
    var body: some View {
        ZStack {
            if let call = calls.call {
                if call.incoming && call.ringing && calls.ringInApp { IncomingCallCard(call: call) }
                else if !(call.incoming && call.ringing) { InCallView(call: call) }
            } else if let last = calls.last {
                CallEndedCard(call: last)
            }
        }
        .animation(.spring(response: 0.3, dampingFraction: 0.9), value: calls.call?.id)
    }
}

private func clock(_ ms: Double) -> String {
    let s = max(0, Int(ms / 1000))
    return s >= 3600 ? String(format: "%d:%02d:%02d", s / 3600, s / 60 % 60, s % 60) : String(format: "%d:%02d", s / 60, s % 60)
}

private struct CallAvatar: View {
    @EnvironmentObject private var bridge: Bridge
    let person: String
    let name: String
    var size: CGFloat = 96
    var body: some View {
        if let friend = bridge.friends.first(where: { $0.id == person }) {
            ContactAvatar(friend: friend, size: size)
        } else {
            Circle().fill(Color.beam.gradient).frame(width: size, height: size)
                .overlay(Text(String(name.prefix(1))).font(.system(size: size * 0.4, weight: .semibold)).foregroundStyle(.white))
        }
    }
}

struct IncomingCallCard: View {
    let call: CallSnapshot
    @ObservedObject private var calls = CallCenter.shared
    var body: some View {
        VStack {
            HStack(spacing: 12) {
                CallAvatar(person: call.person, name: call.name, size: 52)
                VStack(alignment: .leading, spacing: 2) {
                    Text(call.name).font(.headline).lineLimit(1)
                    Text(call.video ? "Video call" : "Audio call").font(.subheadline).foregroundStyle(.secondary)
                }
                Spacer()
                Button { calls.decline() } label: { Image(systemName: "phone.down.fill") }
                    .buttonStyle(CallRoundStyle(tint: .red)).accessibilityLabel("Decline")
                Button { calls.answerInApp() } label: { Image(systemName: call.video ? "video.fill" : "phone.fill") }
                    .buttonStyle(CallRoundStyle(tint: .green)).accessibilityLabel("Answer").disabled(calls.busy)
            }
            .padding(14).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 22))
            .padding(.horizontal, 12)
            Spacer()
        }
        .transition(.move(edge: .top).combined(with: .opacity))
    }
}

struct CallRoundStyle: ButtonStyle {
    var tint: Color
    var size: CGFloat = 50
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.font(.system(size: size * 0.4, weight: .semibold)).foregroundStyle(.white)
            .frame(width: size, height: size).background(tint, in: Circle())
            .opacity(configuration.isPressed ? 0.7 : 1)
    }
}

private struct CallControl: View {
    let icon: String
    let label: String
    var on = false
    let action: () -> Void
    var body: some View {
        Button(action: { Haptics.tap(); action() }) {
            VStack(spacing: 4) {
                Image(systemName: icon).font(.system(size: 22, weight: .medium))
                    .frame(width: 58, height: 58)
                    .foregroundStyle(on ? Color.black : Color.white)
                    .background(on ? Color.white : Color.white.opacity(0.18), in: Circle())
                Text(label).font(.caption2).foregroundStyle(.white)
            }
        }.accessibilityLabel(label).accessibilityAddTraits(on ? .isSelected : [])
    }
}

struct InCallView: View {
    let call: CallSnapshot
    @ObservedObject private var calls = CallCenter.shared
    @State private var now = Date()
    private let tick = Timer.publish(every: 0.5, on: .main, in: .common).autoconnect()

    private var status: String {
        switch call.phase {
        case "ringing": return call.legs.contains { $0.state == "ringing" } ? "Ringing…" : "Calling…"
        case "connecting": return "Connecting…"
        default: return clock(now.timeIntervalSince1970 * 1000 - (call.mediaMs ?? call.answeredMs ?? now.timeIntervalSince1970 * 1000))
        }
    }
    private var recordingBadge: String? {
        let r = call.recording ?? .init()
        if r.meRecording && r.themRecording { return "You’re both recording" }
        if r.themRecording { return "\(call.name) is recording" }
        return nil
    }
    private var pathWords: String? {
        guard let q = calls.quality else { return nil }
        let path = ["local": "Same network", "direct": "Direct", "relay": "Relay"][q.path ?? ""] ?? q.path
        return [path, q.rttMs.map { "\($0) ms" }].compactMap { $0 }.joined(separator: " · ")
    }

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            if calls.remoteVideo != nil && (call.video || calls.remoteCamera) {
                RTCVideo(track: calls.remoteVideo).ignoresSafeArea()
            } else {
                VStack(spacing: 10) {
                    CallAvatar(person: call.person, name: call.name, size: 120)
                    Text(call.name).font(.title2.weight(.semibold)).foregroundStyle(.white)
                    Text(status + (calls.remoteMuted && call.phase == "active" ? " · muted" : "")).font(.callout).monospacedDigit().foregroundStyle(.white.opacity(0.75))
                }
            }
            VStack {
                HStack(alignment: .top) {
                    if calls.remoteVideo != nil && (call.video || calls.remoteCamera) {
                        VStack(alignment: .leading, spacing: 2) {
                            Text(call.name).font(.headline)
                            Text(status).font(.caption).monospacedDigit().opacity(0.8)
                        }.foregroundStyle(.white).shadow(radius: 2)
                    }
                    Spacer()
                    VStack(alignment: .trailing, spacing: 6) {
                        if let badge = recordingBadge {
                            Label(badge, systemImage: "record.circle").font(.caption.weight(.semibold)).foregroundStyle(.white)
                                .padding(.horizontal, 10).padding(.vertical, 5).background(Color.red.opacity(0.9), in: Capsule())
                                .accessibilityAddTraits(.updatesFrequently)
                        }
                        if call.phase == "active", let words = pathWords {
                            HStack(spacing: 5) {
                                Circle().fill(calls.quality?.grade == "poor" ? Color.red : calls.quality?.grade == "ok" ? Color.orange : Color.green).frame(width: 7, height: 7)
                                Text(words)
                            }
                            .font(.caption2).foregroundStyle(.white).padding(.horizontal, 8).padding(.vertical, 4).background(.black.opacity(0.35), in: Capsule())
                            .accessibilityLabel("Call quality: \(calls.quality?.grade ?? "measuring"), \(words)")
                        }
                    }
                }.padding(.horizontal, 16)
                if let r = call.recording, r.askedByThem && !r.iAllow {
                    VStack(spacing: 10) {
                        Text("\(call.name) wants to record this call on their device.").font(.subheadline).multilineTextAlignment(.center)
                        HStack {
                            Button("Don’t Allow") { calls.answerRecordRequest(false) }.buttonStyle(.bordered)
                            Button("Allow") { calls.answerRecordRequest(true) }.buttonStyle(.borderedProminent).tint(.beam)
                        }
                    }
                    .foregroundStyle(.white).padding(14).background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 16))
                    .padding(.horizontal, 20).padding(.top, 8)
                }
                Spacer()
                if let local = calls.localVideo, calls.cameraOn {
                    HStack { Spacer(); RTCVideo(track: local).frame(width: 110, height: 160).clipShape(RoundedRectangle(cornerRadius: 12)).padding(.trailing, 16) }
                }
                HStack(spacing: 18) {
                    CallControl(icon: calls.muted ? "mic.slash.fill" : "mic.fill", label: calls.muted ? "Unmute" : "Mute", on: calls.muted) { calls.toggleMute() }
                    CallControl(icon: calls.cameraOn ? "video.fill" : "video.slash.fill", label: "Camera", on: !calls.cameraOn) { calls.toggleCamera() }
                    if calls.cameraOn {
                        CallControl(icon: "arrow.triangle.2.circlepath.camera", label: "Flip") { calls.switchCamera() }
                    }
                    CallControl(icon: calls.speaker ? "speaker.wave.3.fill" : "speaker.fill", label: "Speaker", on: calls.speaker) { calls.toggleSpeaker() }
                    Button { calls.hangup() } label: { Image(systemName: "phone.down.fill") }
                        .buttonStyle(CallRoundStyle(tint: .red, size: 58)).accessibilityLabel("End call")
                }.padding(.bottom, 24)
            }
        }
        .onReceive(tick) { now = $0 }
        .transition(.opacity)
        .statusBarHidden(call.video)
    }
}

struct CallEndedCard: View {
    let call: CallSnapshot
    @ObservedObject private var calls = CallCenter.shared
    private var words: String {
        let dur = call.mediaMs.flatMap { m in call.endedMs.map { clock($0 - m) } }
        switch call.end {
        case "hangup", "remote_hangup": return dur.map { "Call ended · \($0)" } ?? "Call ended"
        case "declined": return call.incoming ? "Declined" : "\(call.name) declined"
        case "busy": return "\(call.name) is on another call"
        case "no_answer": return "No answer"
        case "missed": return "Missed call"
        case "canceled": return "Canceled"
        case "unavailable": return "Couldn’t reach \(call.name). They may need to open or update DropBeam."
        case "failed": return dur.map { "Call dropped · \($0)" } ?? "Couldn’t connect the call"
        default: return "Call ended"
        }
    }
    var body: some View {
        VStack {
            HStack(spacing: 12) {
                CallAvatar(person: call.person, name: call.name, size: 40)
                VStack(alignment: .leading, spacing: 2) {
                    Text(call.name).font(.subheadline.weight(.semibold)).lineLimit(1)
                    Text(words).font(.caption).foregroundStyle(.secondary)
                }
                Spacer()
                Button { calls.last = nil; calls.startCall(friendId: call.person, video: call.video) } label: {
                    Image(systemName: call.video ? "video.fill" : "phone.fill")
                }.buttonStyle(.bordered).accessibilityLabel("Call back")
                Button { calls.last = nil } label: { Image(systemName: "xmark") }.buttonStyle(.borderless).accessibilityLabel("Close")
            }
            .padding(12).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 18)).padding(.horizontal, 12)
            Spacer()
        }
        .transition(.move(edge: .top).combined(with: .opacity))
        .task(id: call.id) {
            try? await Task.sleep(for: .seconds(4))
            if calls.last?.id == call.id { calls.last = nil }
        }
    }
}

/// A call history line in the thread (desktop: ChatView CallLine). Tap = call back.
struct CallLinePill: View {
    let message: ChatMessage
    let friendID: String
    var body: some View {
        let outcome = message.call?.outcome ?? ""
        let missed = !message.fromMe && (outcome == "missed" || outcome == "declined")
        let video = message.call?.video == true
        let icon = video ? "video.fill" : missed ? "phone.arrow.down.left" : message.fromMe ? "phone.arrow.up.right" : "phone.arrow.down.left"
        Button { CallCenter.shared.startCall(friendId: friendID, video: video) } label: {
            HStack(spacing: 6) {
                Image(systemName: icon).foregroundStyle(missed ? Color.red : Color.green)
                Text(message.text ?? (video ? "Video call" : "Audio call")).foregroundStyle(missed ? Color.red : Color.primary)
                Text(message.date, style: .time).foregroundStyle(.secondary)
            }
            .font(.footnote).padding(.horizontal, 12).padding(.vertical, 6)
            .background(Color(.secondarySystemBackground), in: Capsule())
        }
        .buttonStyle(.plain).frame(maxWidth: .infinity)
        .accessibilityHint(video ? "Starts a video call" : "Starts an audio call")
    }
}
