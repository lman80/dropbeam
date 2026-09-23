import SwiftUI

/// Settings → Devices: every device in this account, kept in sync peer to peer.
struct DevicesView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var linking: LinkStart?
    @State private var removing: AccountDevice?
    @State private var leaving = false
    @State private var syncing = false
    private var devices: [AccountDevice] { bridge.myDevice?.devices ?? [] }
    private var inAccount: Bool { bridge.myDevice?.inAccount == true && devices.count > 1 }
    private var myNoun: String { deviceNoun(bridge.myDevice?.deviceKind, os: bridge.myDevice?.deviceOs ?? "ios") }
    var body: some View {
        ScrollView {
            GlassGroup {
                VStack(alignment: .leading, spacing: 24) {
                    hero
                    if inAccount {
                        VStack(alignment: .leading, spacing: 12) {
                            Text("My Devices").font(.title2.weight(.semibold))
                            GlassCard {
                                VStack(spacing: 0) {
                                    ForEach(Array(devices.enumerated()), id: \.element.id) { index, device in
                                        if index > 0 { Divider().padding(.leading, 62) }
                                        row(device)
                                    }
                                }
                            }
                            if let name = bridge.settings?.displayName, !name.isEmpty {
                                Text("Friends see you as **\(name)** on every device. Change your name or photo on any of them and the others follow.")
                                    .font(.footnote).foregroundStyle(.secondary)
                            }
                        }
                        Button { linking = .show; Haptics.tap() } label: { Label("Link a Device", systemImage: "plus").frame(maxWidth: .infinity, minHeight: 36) }.beamButton(prominent: true)
                        Button { syncNow() } label: { Label(syncing ? "Syncing…" : "Sync Now", systemImage: "arrow.triangle.2.circlepath").frame(maxWidth: .infinity, minHeight: 36) }.beamButton().disabled(syncing)
                        Button(role: .destructive) { leaving = true } label: { Text("Remove This \(myNoun) from Account").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                    } else {
                        GlassCard {
                            VStack(alignment: .leading, spacing: 14) {
                                Label("Use DropBeam on another device?", systemImage: "laptopcomputer.and.iphone").font(.headline)
                                Text("Link your Mac, PC or another phone and each one gets your friends and chats right away — then everything stays in sync.").foregroundStyle(.secondary)
                                Button { linking = .show; Haptics.tap() } label: { Label("Link a Device", systemImage: "plus").frame(maxWidth: .infinity, minHeight: 36) }.beamButton(prominent: true)
                                Button { linking = .scan; Haptics.tap() } label: { Label("Scan the Other Device's Code", systemImage: "qrcode.viewfinder").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                            }
                        }
                    }
                    Text("Your friends, conversations, name and photo sync directly between your devices — end-to-end encrypted, never stored on a server.")
                        .font(.footnote).foregroundStyle(.secondary)
                }.padding(20)
            }
        }
        .contentMargins(.bottom, 24, for: .scrollContent)
        .navigationTitle("Devices").navigationBarTitleDisplayMode(.large).beamCanvas()
        .refreshable { await refresh(sync: true) }
        .task { await refresh(sync: false) }
        .sheet(item: $linking, onDismiss: { Task { await refresh(sync: false) } }) { start in LinkDeviceSheet(start: start, title: "Link a Device") }
        .confirmationDialog(removing.map { "Remove \(label($0))?" } ?? "", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible, presenting: removing) { device in
            Button("Remove from Account", role: .destructive) { bridge.perform { try await bridge.accountRemoveDevice(endpointId: device.endpointId); bridge.showToast("\(label(device)) was removed from your account") } }
        } message: { _ in Text("It stops getting your friends and chats, and it's told the next time it's online. You can link it again later.") }
        .confirmationDialog("Remove this \(myNoun) from your account?", isPresented: $leaving, titleVisibility: .visible) {
            Button("Remove", role: .destructive) { bridge.perform { try await bridge.accountLeave(); bridge.showToast("This \(myNoun) left your account") } }
        } message: { Text("Your friends and chats stay on this \(myNoun), but stop syncing with your other devices. Devices that are offline are told the next time they see this one.") }
    }
    private var hero: some View {
        HStack(spacing: 14) {
            ForEach(Array(devices.prefix(3).enumerated()), id: \.element.id) { _, d in
                DeviceAvatar(kind: d.deviceKind, os: d.deviceOs, size: 56)
            }
            if devices.isEmpty { DeviceAvatar(kind: "phone", os: "ios", size: 56) }
        }.frame(maxWidth: .infinity).padding(.top, 4)
    }
    /// "Your iPhone" — or "Your iPhone 2" when two of your devices would read the
    /// same (mirrors ownDeviceLabels in src/lib/deviceIcons.ts).
    private func label(_ device: AccountDevice) -> String {
        let noun = deviceNoun(device.deviceKind, os: device.deviceOs)
        if device.thisDevice { return "This \(noun)" }
        let same = devices.filter { !$0.thisDevice && deviceNoun($0.deviceKind, os: $0.deviceOs) == noun }
        guard same.count > 1 else { return "Your \(noun)" }
        let names = same.map { $0.name.trimmingCharacters(in: .whitespaces) }
        if Set(names).count == names.count, !names.contains(noun), !names.contains("") { return device.name }
        let index = (same.map(\.endpointId).sorted().firstIndex(of: device.endpointId) ?? 0) + 1
        return "Your \(noun) \(index)"
    }
    @ViewBuilder private func row(_ device: AccountDevice) -> some View {
        HStack(spacing: 14) {
            DeviceAvatar(kind: device.deviceKind, os: device.deviceOs, size: 46)
            VStack(alignment: .leading, spacing: 3) {
                Text(label(device)).font(.headline)
                Text(subtitle(device)).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: 4)
            if !device.thisDevice {
                Menu {
                    Button("Remove from Account", systemImage: "minus.circle", role: .destructive) { removing = device }
                } label: { Image(systemName: "ellipsis").frame(width: 36, height: 36).contentShape(Rectangle()) }
                    .accessibilityLabel("Options for \(label(device))")
            }
        }
        .frame(minHeight: 60)
        .contextMenu { if !device.thisDevice { Button("Remove from Account", systemImage: "minus.circle", role: .destructive) { removing = device } } }
    }
    private func subtitle(_ device: AccountDevice) -> String {
        if device.thisDevice { return device.name }
        let online = device.friendId.map { bridge.presence[$0] == true } ?? false
        var parts = [device.name, online ? "Online" : "Offline"]
        if let ms = device.lastSyncMs, ms > 0 {
            let date = Date(timeIntervalSince1970: ms / 1000)
            parts.append("Synced " + (Date().timeIntervalSince(date) < 60 ? "just now" : date.formatted(.relative(presentation: .named))))
        } else { parts.append("Not synced yet") }
        return parts.joined(separator: " · ")
    }
    private func syncNow() {
        syncing = true; Haptics.tap()
        Task { await refresh(sync: true); try? await Task.sleep(for: .seconds(2)); try? await bridge.action("myDeviceInfo"); syncing = false }
    }
    private func refresh(sync: Bool) async {
        if sync { try? await bridge.accountSyncNow() }
        try? await bridge.action("myDeviceInfo")
    }
}

/// Where a linking sheet opens: on this device's code, or in the camera.
enum LinkStart: String, Identifiable { case show, scan; var id: String { rawValue } }

/// What the bridge answers after a link (title/detail already worded).
private struct LinkOutcome: Decodable {
    var name: String?
    var deviceKind: String?
    var deviceOs: String?
    var friends: Int?
    var messages: Int?
    var title: String?
    var detail: String?
}

/// The words for a link, mirroring src/lib/deviceLink.ts.
private enum LinkWords {
    static func count(_ n: Int, _ one: String) -> String { "\(n.formatted()) \(n == 1 ? one : one + "s")" }
    static func summary(_ friends: Int?, _ messages: Int?) -> String? {
        let parts = [friends.flatMap { $0 > 0 ? count($0, "friend") : nil }, messages.flatMap { $0 > 0 ? count($0, "message") : nil }].compactMap { $0 }
        return parts.isEmpty ? nil : parts.joined(separator: " and ")
    }
    static func progress(_ p: [String: Any]?) -> String {
        guard let p else { return "Connecting to your other device…" }
        let what = summary(p["friends"] as? Int, p["messages"] as? Int)
        switch p["stage"] as? String {
        case "waiting": return "Connected — waiting for your other device…"
        case "sending": return what.map { "Sending \($0)…" } ?? "Linking…"
        default: return what.map { "Bringing over \($0)…" } ?? "Setting up this device…"
        }
    }
    static func title(kind: String?, os: String?, name: String?) -> String {
        if kind == nil && os == nil { return name.map { "Linked with \($0)" } ?? "Your devices are linked" }
        return "Linked with your \(deviceNoun(kind, os: os))"
    }
    static func detail(_ friends: Int?, _ messages: Int?) -> String {
        let tail = "From here on your friends, chats, name and photo stay in sync."
        guard let what = summary(friends, messages) else { return tail }
        let one = (friends ?? 0) + (messages ?? 0) == 1
        return what.prefix(1).uppercased() + what.dropFirst() + (one ? " is" : " are") + " on both devices now. " + tail
    }
}

/// The one linking flow: this device shows its code AND can scan the other's.
/// Either device scanning the other works; the account that already has
/// devices is the one both end up in, and two different accounts are refused
/// before anything changes.
struct LinkDeviceSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let start: LinkStart
    let title: String
    private enum Phase: Equatable { case show, working, done, failed }
    @State private var phase: Phase = .show
    /// How the last attempt was made: this device scanning, or showing its code.
    @State private var via: LinkStart = .show
    @State private var code = ""
    @State private var codeError: String?
    @State private var progress: [String: Any]?
    @State private var doneTitle = ""
    @State private var doneDetail = ""
    @State private var error = ""
    @State private var scanning = false
    @State private var closed = false
    @State private var attempt = 0
    /// A device that already shares its account shows a "join me" code.
    private var hosting: Bool { (bridge.myDevice?.devices.count ?? 0) > 1 }
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 22) {
                    switch phase {
                    case .show: showing
                    case .working:
                        GlassCard {
                            VStack(spacing: 16) {
                                ProgressView().controlSize(.large)
                                Text(LinkWords.progress(progress)).font(.headline).multilineTextAlignment(.center)
                                Text("Keep DropBeam open on both devices.").font(.subheadline).foregroundStyle(.secondary)
                            }.frame(maxWidth: .infinity).padding(.vertical, 12)
                        }.accessibilityElement(children: .combine)
                    case .done:
                        BeamEmpty(symbol: "checkmark.circle.fill", title: doneTitle, detail: doneDetail)
                        Button { dismiss() } label: { Text("Done").frame(maxWidth: .infinity, minHeight: 36) }.beamButton(prominent: true)
                    case .failed:
                        GlassCard {
                            VStack(alignment: .leading, spacing: 14) {
                                Label("Couldn't link", systemImage: "exclamationmark.triangle.fill").font(.headline).foregroundStyle(.red)
                                Text(error).foregroundStyle(.secondary)
                            }
                        }
                        Button { retry() } label: { Text("Try Again").frame(maxWidth: .infinity, minHeight: 36) }.beamButton(prominent: true)
                        if via == .scan {
                            Button { error = ""; phase = .show; attempt += 1 } label: { Label("Show This Device's Code Instead", systemImage: "qrcode").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                        } else {
                            Button { error = ""; phase = .show; via = .scan; scanning = true } label: { Label("Scan the Other Device Instead", systemImage: "qrcode.viewfinder").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                        }
                    }
                }.padding(20)
            }
            .navigationTitle(phase == .done ? "Devices Linked" : title).navigationBarTitleDisplayMode(.inline).beamCanvas()
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button(phase == .done ? "Close" : phase == .working ? "Hide" : "Cancel") { dismiss() } } }
            // A fresh one-time code each time the code screen opens (or on Try Again).
            .task(id: attempt) { if phase == .show { await begin() } }
            // Opened to scan: the camera goes on top of this device's own code,
            // so closing the camera leaves "let the other device scan this" ready.
            .onAppear { if start == .scan { via = .scan; scanning = true } }
            .onReceive(NotificationCenter.default.publisher(for: Notification.Name("DropBeam.link://progress"))) { note in
                guard phase == .show || phase == .working else { return }
                progress = note.userInfo?["payload"] as? [String: Any]
                if phase == .show { via = .show; scanning = false; withAnimation(.smooth) { phase = .working } }
            }
            .onReceive(NotificationCenter.default.publisher(for: Notification.Name("DropBeam.link://linked"))) { note in
                guard phase != .done else { return }
                let p = note.userInfo?["payload"] as? [String: Any]
                finish(title: LinkWords.title(kind: p?["device_kind"] as? String, os: p?["device_os"] as? String, name: p?["name"] as? String),
                       detail: LinkWords.detail(p?["friends"] as? Int, p?["messages"] as? Int))
            }
            .onReceive(NotificationCenter.default.publisher(for: Notification.Name("DropBeam.link://failed"))) { note in
                guard phase != .done else { return }
                fail((note.userInfo?["payload"] as? String) ?? "Your other device couldn't finish linking.")
            }
            .sheet(isPresented: $scanning) {
                QRScannerSheet(title: "Scan Your Other Device", autoSubmit: true) { value in
                    // Say what was scanned instead, and keep the camera open.
                    if !Bridge.isLinkCode(value) {
                        throw NSError(domain: "DropBeam", code: 1, userInfo: [NSLocalizedDescriptionKey: value.lowercased().hasPrefix("dropbeam") ?
                            "That's a friend code. To link your own devices, scan the code in Settings → Devices on your other device." :
                            "That isn't a DropBeam device code. On your other device open Settings → Devices → Link a Device."])
                    }
                    via = .scan; progress = nil; error = ""
                    withAnimation(.smooth) { phase = .working }
                    scanning = false
                    Task { await link(value) }
                }
            }
            .onDisappear { closed = true; Task { try? await bridge.linkHostCancel(); try? await bridge.linkDeviceCancel() } }
        }.interactiveDismissDisabled(phase == .working)
    }
    @ViewBuilder private var showing: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("1. Open DropBeam on your other device.")
            Text("2. Go to **Settings → Devices → Link a Device** — on a phone you're just setting up, tap **Already use DropBeam?**")
            Text("3. Scan this code.")
        }.frame(maxWidth: .infinity, alignment: .leading).foregroundStyle(.secondary)
        GlassCard {
            VStack(spacing: 16) {
                if !code.isEmpty { InviteQRCode(code: code) }
                else if let codeError { Text(codeError).foregroundStyle(.red); Button("Try Again") { attempt += 1 }.beamButton() }
                else { ProgressView().frame(height: 240) }
                Label("Waiting for your other device…", systemImage: "antenna.radiowaves.left.and.right").font(.subheadline).foregroundStyle(.secondary).opacity(code.isEmpty ? 0 : 1)
            }.frame(maxWidth: .infinity)
        }
        Button { via = .scan; scanning = true; Haptics.tap() } label: { Label("Scan the Other Device's Code Instead", systemImage: "qrcode.viewfinder").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
        if !code.isEmpty { Button("Copy Code") { UIPasteboard.general.string = code; Haptics.tap(); bridge.showToast("Code copied") }.font(.subheadline) }
        Text("Either device can scan the other. The code works once, for 10 minutes. Your friends and chats come along, and nothing on either device is lost.")
            .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
    }
    private func begin() async {
        code = ""; codeError = nil
        do {
            let next = try await (hosting ? bridge.linkHostBegin() : bridge.linkDeviceBegin())
            if closed { try? await bridge.linkHostCancel(); try? await bridge.linkDeviceCancel() } else { code = next }
        } catch { codeError = "Couldn't create a code. \(error.localizedDescription)" }
    }
    private func link(_ value: String) async {
        do {
            let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
            let outcome: LinkOutcome = try await bridge.call(trimmed.lowercased().hasPrefix("dropbeamjoin1:") ? "linkDeviceJoin" : "linkDeviceSend", ["code": trimmed])
            finish(title: outcome.title ?? LinkWords.title(kind: outcome.deviceKind, os: outcome.deviceOs, name: outcome.name),
                   detail: outcome.detail ?? LinkWords.detail(outcome.friends, outcome.messages))
        } catch { if phase != .done { fail(error.localizedDescription) } }
    }
    private func finish(title: String, detail: String) {
        doneTitle = title; doneDetail = detail; scanning = false
        withAnimation(.smooth) { phase = .done }
        UINotificationFeedbackGenerator().notificationOccurred(.success)
    }
    private func fail(_ message: String) {
        error = message; scanning = false
        withAnimation(.smooth) { phase = .failed }
        UINotificationFeedbackGenerator().notificationOccurred(.error)
    }
    private func retry() {
        error = ""; progress = nil; phase = .show
        if via == .scan { scanning = true } else { attempt += 1 }
    }
}

/// On the device that HAS the account: show a code the new device scans.
struct AddDeviceSheet: View {
    var body: some View { LinkDeviceSheet(start: .show, title: "Link a Device") }
}

/// On a NEW device (first run → "Already use DropBeam?"): straight to the camera,
/// with this device's own code underneath as the way back.
struct JoinAccountSheet: View {
    var body: some View { LinkDeviceSheet(start: .scan, title: "Link to Your Account") }
}
