import SwiftUI

/// What the user's other linked devices are sending and receiving right now
/// (GitHub #31), mirrored from the engine's own-device digest. Read-only — no
/// cancel or pause — and absent while nothing is happening elsewhere.
struct DeviceTransfer: Decodable, Identifiable, Equatable {
    let id: String
    let direction: String
    let state: String
    var names: [String] = []
    var fileCount: Double = 0
    var bytesDone: Double = 0
    var bytesTotal: Double = 0
    var percent: Double = 0
    var speedBps: Double = 0
    var peer: String?
    private enum CodingKeys: String, CodingKey { case id, direction, state, names, fileCount, bytesDone, bytesTotal, percent, speedBps, peer }
    init(from d: Decoder) throws {
        let c = try d.container(keyedBy: CodingKeys.self)
        id = try c.decode(String.self, forKey: .id)
        direction = try c.decode(String.self, forKey: .direction)
        state = try c.decode(String.self, forKey: .state)
        names = try c.decodeIfPresent([String].self, forKey: .names) ?? []
        fileCount = try c.decodeIfPresent(Double.self, forKey: .fileCount) ?? Double(names.count)
        bytesDone = try c.decodeIfPresent(Double.self, forKey: .bytesDone) ?? 0
        bytesTotal = try c.decodeIfPresent(Double.self, forKey: .bytesTotal) ?? 0
        percent = try c.decodeIfPresent(Double.self, forKey: .percent) ?? 0
        speedBps = try c.decodeIfPresent(Double.self, forKey: .speedBps) ?? 0
        peer = try c.decodeIfPresent(String.self, forKey: .peer)
    }
    init(id: String, direction: String, state: String, names: [String], fileCount: Double, bytesDone: Double, bytesTotal: Double, percent: Double, speedBps: Double, peer: String?) {
        self.id = id; self.direction = direction; self.state = state; self.names = names; self.fileCount = fileCount
        self.bytesDone = bytesDone; self.bytesTotal = bytesTotal; self.percent = percent; self.speedBps = speedBps; self.peer = peer
    }
    var done: Bool { state == "completed" }
    var failed: Bool { state == "failed" || state == "canceled" }
    var moving: Bool { state == "transferring" && bytesTotal > 0 }
    var title: String {
        guard let first = names.first else { let n = Int(fileCount); return "\(n) file\(n == 1 ? "" : "s")" }
        return fileCount > 1 ? "\(first) + \(Int(fileCount) - 1) more" : first
    }
    /// "Sending to Alex", "Received from Alex", "Waiting for Alex" …
    var line: String {
        let send = direction == "send"
        let who = peer.map { (send ? " to " : " from ") + $0 } ?? ""
        switch state {
        case "completed": return (send ? "Sent" : "Received") + who
        case "failed": return (send ? "Couldn’t send" : "Couldn’t receive") + who
        case "canceled": return "Canceled"
        case "paused": return "Paused"
        case "held": return "Waiting on a Transfer Server" + who
        case "waitingForPeer": return peer.map { "Waiting for \($0)" } ?? "Waiting for the other device"
        case "waitingForAccept": return peer.map { "Waiting for \($0) to accept" } ?? "Waiting to be accepted"
        case "starting", "connecting": return (send ? "Getting ready to send" : "Getting ready to receive") + who
        default: return (send ? "Sending" : "Receiving") + who
        }
    }
}

struct DeviceActivity: Decodable, Identifiable, Equatable {
    var id: String { endpointId }
    let endpointId: String
    let name: String
    var kind: String?
    var os: String?
    var items: [DeviceTransfer] = []
}

struct OtherDevicesSection: View {
    @EnvironmentObject private var bridge: Bridge
    var body: some View {
        if !bridge.otherDevices.isEmpty {
            Section {
                ForEach(bridge.otherDevices) { device in
                    ForEach(device.items) { item in OtherDeviceRow(device: device, item: item) }
                }
            } header: { Text("On Your Other Devices") }.headerProminence(.increased)
        }
    }
}

private struct OtherDeviceRow: View {
    let device: DeviceActivity
    let item: DeviceTransfer
    private var symbol: String {
        deviceSymbol(device.kind) ?? (device.os == "ios" ? "iphone" : device.os == "macos" ? "laptopcomputer" : "desktopcomputer")
    }
    var body: some View {
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 12) {
                Image(systemName: symbol).font(.title3).foregroundStyle(.secondary)
                    .frame(width: 44, height: 44)
                    .background(Color(uiColor: .tertiarySystemFill), in: RoundedRectangle(cornerRadius: 12, style: .continuous))
                    .overlay(alignment: .bottomTrailing) {
                        if item.done || item.failed {
                            Image(systemName: item.done ? "checkmark.circle.fill" : "xmark.circle.fill")
                                .font(.system(size: 15, weight: .bold)).symbolRenderingMode(.palette)
                                .foregroundStyle(.white, item.done ? Color.green : Color.red)
                                .background(Circle().fill(Color(uiColor: .secondarySystemGroupedBackground)).padding(-2))
                                .offset(x: 5, y: 5)
                        }
                    }
                VStack(alignment: .leading, spacing: 2) {
                    Text(item.title).font(.body.weight(.semibold)).lineLimit(1).truncationMode(.middle)
                    (Text(device.name).foregroundStyle(.primary) + Text(" · \(item.line)"))
                        .font(.subheadline).foregroundStyle(.secondary).lineLimit(2)
                }
                Spacer(minLength: 4)
                if item.moving {
                    Text("\(Int(item.percent))%").font(.subheadline).foregroundStyle(.secondary).monospacedDigit()
                }
            }
            if item.moving {
                VStack(alignment: .leading, spacing: 5) {
                    ProgressView(value: min(100, max(0, item.percent)), total: 100).tint(.beam)
                    Text("\(Formatters.bytes(item.bytesDone)) of \(Formatters.bytes(item.bytesTotal))\(item.speedBps > 0 ? " · " + Formatters.speed(item.speedBps) : "")")
                        .font(.caption).foregroundStyle(.secondary).monospacedDigit().lineLimit(1)
                }
            }
        }
        .padding(.vertical, 4)
        .accessibilityElement(children: .combine)
        .accessibilityHint("On \(device.name). Shown for information only.")
    }
}
