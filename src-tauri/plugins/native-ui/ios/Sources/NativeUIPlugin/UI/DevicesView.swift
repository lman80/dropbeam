import SwiftUI

/// Settings → Devices: every device in this account, kept in sync peer to peer.
struct DevicesView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var linking: LinkStart?
    @State private var removing: AccountDevice?
    @State private var approving: AccountDevice?
    @State private var leaving = false
    @State private var syncing = false
    private var devices: [AccountDevice] { bridge.myDevice?.devices ?? [] }
    /// Devices actually in the account (not ones still waiting for approval).
    private var linkedDevices: [AccountDevice] { bridge.myDevice?.linked ?? [] }
    private var inAccount: Bool { bridge.myDevice?.inAccount == true && devices.count > 1 }
    private var myNoun: String { deviceNoun(bridge.myDevice?.deviceKind, os: bridge.myDevice?.deviceOs ?? "ios") }
    var body: some View {
        List {
            Section { hero }.clearRow(EdgeInsets(top: 0, leading: 20, bottom: 4, trailing: 20))
            Section {} footer: {
                Text("Link your phone and computers so they’re all you: the same friends, chats, name and photo on each.")
                    .frame(maxWidth: .infinity).multilineTextAlignment(.center)
            }
            if inAccount {
                Section {
                    ForEach(linkedDevices) { device in row(device) }
                    ForEach(devices.filter(\.needsApproval)) { device in pendingRow(device) }
                } header: { Text("My Devices") } footer: {
                    if let name = bridge.settings?.displayName, !name.isEmpty {
                        Text("Friends see you as \(name) on every device. A new name or photo on one updates the others.")
                    }
                }
                Section {
                    Button { linking = .show; Haptics.tap() } label: { Label("Link a Device", systemImage: "plus.circle") }
                    Button { syncNow() } label: {
                        HStack { Label("Sync Now", systemImage: "arrow.triangle.2.circlepath"); Spacer(); if syncing { ProgressView() } }
                    }.disabled(syncing)
                }
                Section {
                    Button("Remove This \(myNoun) from My Devices", role: .destructive) { leaving = true }
                } footer: { Text("Lost a phone or computer? Remove it above (swipe or tap ⋯) so it gets no new messages. Messages already on it stay there, so also lock or erase it with Find My.\n\nFriends, chats, your name and photo sync directly between your devices, end-to-end encrypted.") }
            } else {
                Section {
                    Button { linking = .show; Haptics.tap() } label: { Label("Link a Device", systemImage: "plus.circle") }
                    Button { linking = .scan; Haptics.tap() } label: { Label("Scan the Other Device’s Code", systemImage: "qrcode.viewfinder") }
                } header: { Text("Use DropBeam on Another Device") } footer: {
                    Text("Link your Mac, PC or another phone to share your friends and chats. They stay in sync directly between your devices, end-to-end encrypted.\n\nYour friends and chats are only on your devices — there’s no online backup. Link a second device so losing one doesn’t lose them.")
                }
            }
        }
        .beamList()
        .navigationTitle("Devices").navigationBarTitleDisplayMode(.large)
        .refreshable { await refresh(sync: true) }
        .task { await refresh(sync: false) }
        .sheet(item: $linking, onDismiss: { Task { await refresh(sync: false) } }) { start in LinkDeviceSheet(start: start, title: "Link a Device") }
        .confirmationDialog(removing.map { "Remove \(label($0))?" } ?? "", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible, presenting: removing) { device in
            Button("Remove Device", role: .destructive) { bridge.perform { try await bridge.accountRemoveDevice(endpointId: device.endpointId); bridge.showToast("\(label(device)) was removed from your devices") } }
        } message: { _ in Text("It stops getting your new messages and friends, and it’s told the next time it’s online. What’s already on it stays there. You can link it again later.") }
        .confirmationDialog(approving.map { "Is “\($0.name)” yours?" } ?? "", isPresented: Binding(get: { approving != nil }, set: { if !$0 { approving = nil } }), titleVisibility: .visible, presenting: approving) { device in
            Button("Yes, It’s Mine") { approve(device) }
            Button("No — Remove It", role: .destructive) { removing = device }
        } message: { _ in Text("Only say yes if you set up DropBeam on this device yourself. Saying yes gives it all your friends and chats. Not sure? Remove it — you can link it again later.") }
        .confirmationDialog("Remove this \(myNoun) from your devices?", isPresented: $leaving, titleVisibility: .visible) {
            Button("Remove", role: .destructive) { bridge.perform { try await bridge.accountLeave(); bridge.showToast("This \(myNoun) is no longer linked to your other devices") } }
        } message: { Text("Your friends and chats stay on this \(myNoun), but stop syncing with your other devices. Devices that are offline are told the next time they see this one.") }
    }
    private var hero: some View {
        HStack(spacing: -8) {
            ForEach(Array(linkedDevices.prefix(3).enumerated()), id: \.element.id) { _, d in
                DeviceAvatar(kind: d.deviceKind, os: d.deviceOs, size: 64)
                    .overlay(Circle().stroke(Color(uiColor: .systemGroupedBackground), lineWidth: 3))
            }
            if linkedDevices.isEmpty { DeviceAvatar(kind: "phone", os: "ios", size: 64) }
        }.frame(maxWidth: .infinity)
    }
    /// "Your iPhone" — or "Your iPhone 2" when two of your devices would read the
    /// same (mirrors ownDeviceLabels in src/lib/deviceIcons.ts).
    private func label(_ device: AccountDevice) -> String {
        let noun = deviceNoun(device.deviceKind, os: device.deviceOs)
        if device.thisDevice { return "This \(noun)" }
        if device.needsApproval { return device.name }
        let same = linkedDevices.filter { !$0.thisDevice && deviceNoun($0.deviceKind, os: $0.deviceOs) == noun }
        guard same.count > 1 else { return "Your \(noun)" }
        let names = same.map { $0.name.trimmingCharacters(in: .whitespaces) }
        if Set(names).count == names.count, !names.contains(noun), !names.contains("") { return device.name }
        let index = (same.map(\.endpointId).sorted().firstIndex(of: device.endpointId) ?? 0) + 1
        return "Your \(noun) \(index)"
    }
    @ViewBuilder private func row(_ device: AccountDevice) -> some View {
        HStack(spacing: 14) {
            DeviceAvatar(kind: device.deviceKind, os: device.deviceOs, size: 42)
            VStack(alignment: .leading, spacing: 2) {
                Text(label(device)).font(.body.weight(.semibold)).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                Text(subtitle(device)).font(.subheadline).foregroundStyle(.secondary).lineLimit(2)
            }
            Spacer(minLength: 4)
            if !device.thisDevice {
                Menu {
                    Button("Remove Device", systemImage: "minus.circle", role: .destructive) { removing = device }
                } label: { Image(systemName: "ellipsis.circle").font(.title3).frame(width: 44, height: 44).contentShape(Rectangle()) }
                    .buttonStyle(.borderless).accessibilityLabel("Options for \(label(device))")
            }
        }
        .padding(.vertical, 2)
        .swipeActions { if !device.thisDevice { Button("Remove", role: .destructive) { removing = device } } }
        .contextMenu { if !device.thisDevice { Button("Remove Device", systemImage: "minus.circle", role: .destructive) { removing = device } } }
    }
    /// S4: a device that proves the account key but no linked device vouched for
    /// (linked by an older build, or by a device since removed). Approve or remove.
    @ViewBuilder private func pendingRow(_ device: AccountDevice) -> some View {
        HStack(alignment: .top, spacing: 14) {
            DeviceAvatar(kind: device.deviceKind, os: device.deviceOs, size: 42).opacity(0.6)
            VStack(alignment: .leading, spacing: 4) {
                Text(device.name).font(.body.weight(.semibold)).lineLimit(2)
                    .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                Text("Needs approval. It says it’s yours, but none of your devices added it. Don’t recognize it? Remove it.")
                    .font(.subheadline).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
                HStack(spacing: 10) {
                    Button("Remove", role: .destructive) { removing = device }.buttonStyle(.borderedProminent).tint(.red)
                    Button("It’s Mine…") { approving = device }.buttonStyle(.bordered)
                }.controlSize(.small).padding(.top, 4)
            }
        }
        .padding(.vertical, 2)
        .swipeActions { Button("Remove", role: .destructive) { removing = device } }
        .contextMenu {
            Button("Approve…", systemImage: "checkmark.circle") { approving = device }
            Button("Remove Device", systemImage: "minus.circle", role: .destructive) { removing = device }
        }
    }
    private func approve(_ device: AccountDevice) {
        Haptics.tap()
        bridge.perform {
            try await bridge.accountApproveDevice(endpointId: device.endpointId)
            try? await bridge.action("myDeviceInfo")
            bridge.showToast("\(device.name) was approved")
        }
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
    private enum Phase: Equatable { case show, working, confirm, done, failed, mismatch }
    @State private var phase: Phase = .show
    /// The safety code both devices show before anything is linked (S1).
    @State private var safety: LinkSafety?
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
    private var hosting: Bool { (bridge.myDevice?.linked.count ?? 0) > 1 }
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
                    case .confirm:
                        if let safety { LinkSafetyCheck(safety: safety, confirm: { confirmLink(safety) }, cancel: { cancelLink(safety) }, mismatch: { mismatch(safety) }) }
                    case .mismatch:
                        BeamEmpty(symbol: "xmark.shield.fill", title: "Nothing Was Linked",
                                  detail: "If the numbers were different, a device that isn’t yours may have scanned your code. Nothing changed on either device. Put your two devices side by side and start again. If the numbers are different again, stop and ask someone you trust for help.")
                        Button { retry() } label: { Text("Start Again").frame(maxWidth: .infinity, minHeight: 36) }.beamButton(prominent: true)
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
            .navigationBarTitleDisplayMode(.inline).beamCanvas()
            .navigationTitle(phase == .done ? "Devices Linked" : phase == .confirm ? "Check the Numbers" : phase == .mismatch ? "Not Linked" : title)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button(phase == .done ? "Close" : "Cancel") { dismiss() } } }
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
            // The other device scanned this one's code: compare safety codes before linking.
            .onReceive(NotificationCenter.default.publisher(for: Notification.Name("DropBeam.link://confirm"))) { note in
                guard let request = LinkConfirmRequest(note.userInfo?["payload"]) else { return }
                // Busy with our own scan (or already done): this request isn't ours to answer yes.
                guard phase == .show || (phase == .working && via == .show) || phase == .failed else {
                    Task { try? await bridge.linkConfirm(endpointId: request.endpointId, accept: false) }
                    return
                }
                via = .show; scanning = false; error = ""
                safety = LinkSafety(name: request.name, safety: request.safety, joining: request.joining, peerShowsCode: true, endpointId: request.endpointId, code: nil)
                withAnimation(.smooth) { phase = .confirm }
                UINotificationFeedbackGenerator().notificationOccurred(.warning)
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
            .onDisappear {
                closed = true
                let unanswered = phase == .confirm ? safety?.endpointId : nil
                Task {
                    if let unanswered { try? await bridge.linkConfirm(endpointId: unanswered, accept: false) }
                    try? await bridge.linkHostCancel(); try? await bridge.linkDeviceCancel()
                }
            }
        }.interactiveDismissDisabled(phase == .working || phase == .confirm)
    }
    @ViewBuilder private var showing: some View {
        VStack(alignment: .leading, spacing: 8) {
            Text("1. Open DropBeam on your other device.")
            Text("2. Go to **Settings → Devices → Link a Device**. On a phone you’re just setting up, tap **I Already Use DropBeam**.")
            Text("3. Choose **Scan the Other Device** and point it at this code.")
        }.frame(maxWidth: .infinity, alignment: .leading).foregroundStyle(.secondary)
        GlassCard {
            VStack(spacing: 16) {
                if !code.isEmpty { InviteQRCode(code: code) }
                else if let codeError { Text(codeError).foregroundStyle(.red); Button("Try Again") { attempt += 1 }.beamButton() }
                else { ProgressView().frame(height: 240) }
                Label("Waiting for your other device…", systemImage: "antenna.radiowaves.left.and.right").font(.subheadline).foregroundStyle(.secondary).opacity(code.isEmpty ? 0 : 1)
            }.frame(maxWidth: .infinity)
        }
        Button { via = .scan; scanning = true; Haptics.tap() } label: { Label("Scan the Other Device", systemImage: "qrcode.viewfinder").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
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
    /// Step 1 of linking with a scanned code: who it is and the safety code to compare.
    /// Nothing is linked until the user confirms the codes match.
    private func link(_ value: String) async {
        let trimmed = value.trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            let preview = try await bridge.linkDevicePrepare(code: trimmed)
            guard !closed, phase == .working, via == .scan else { return }
            safety = LinkSafety(name: preview.name.isEmpty ? "Your other device" : preview.name, safety: preview.safety,
                                joining: preview.direction == "take", peerShowsCode: preview.peerShowsCode, endpointId: nil, code: trimmed)
            withAnimation(.smooth) { phase = .confirm }
        } catch { if phase != .done { fail(error.localizedDescription) } }
    }
    private func confirmLink(_ s: LinkSafety) {
        Haptics.tap(); progress = nil
        withAnimation(.smooth) { phase = .working }
        if let code = s.code {
            // Step 2 (this device scanned): the engine only links a prepared code.
            Task {
                do {
                    let outcome: LinkOutcome = try await bridge.call("linkDeviceSend", ["code": code])
                    finish(title: outcome.title ?? LinkWords.title(kind: outcome.deviceKind, os: outcome.deviceOs, name: outcome.name),
                           detail: outcome.detail ?? LinkWords.detail(outcome.friends, outcome.messages))
                } catch { if phase != .done { fail(error.localizedDescription) } }
            }
        } else if let eid = s.endpointId {
            // The other device scanned ours: say yes; progress/linked/failed events follow.
            Task {
                do { try await bridge.linkConfirm(endpointId: eid, accept: true) }
                catch { if phase != .done { fail(error.localizedDescription) } }
            }
        }
    }
    private func cancelLink(_ s: LinkSafety) {
        Haptics.tap(); safety = nil
        if let eid = s.endpointId {
            // Refused: this code no longer works (a link://failed follows); offer a fresh one.
            Task { try? await bridge.linkConfirm(endpointId: eid, accept: false) }
            fail("Linking was canceled, so that code no longer works. Show a new code to try again.")
        } else {
            Task { try? await bridge.linkDeviceCancel(); try? await bridge.linkHostCancel() }
            fail("Linking was canceled. Nothing was changed on either device.")
        }
    }
    /// "No, they're different": stop, and say what that means.
    private func mismatch(_ s: LinkSafety) {
        Haptics.warning(); safety = nil
        if let eid = s.endpointId { Task { try? await bridge.linkConfirm(endpointId: eid, accept: false) } }
        else { Task { try? await bridge.linkDeviceCancel(); try? await bridge.linkHostCancel() } }
        via = .show; scanning = false
        withAnimation(.smooth) { phase = .mismatch }
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
        let wasMismatch = phase == .mismatch
        error = ""; progress = nil; phase = .show
        if via == .scan && !wasMismatch { scanning = true } else { attempt += 1 }
    }
}

/// The confirmation both linking devices show: the same safety code on each.
struct LinkSafety: Equatable {
    var name: String
    var safety: String
    /// This device joins `name`'s account (else `name` gets this account).
    var joining: Bool
    var peerShowsCode: Bool
    /// Set when the other device scanned ours (answer with linkConfirm).
    var endpointId: String?
    /// Set when this device scanned theirs (continue with linkDeviceSend).
    var code: String?
}

/// "Make sure <name> shows this code" — compare, then link or cancel.
struct LinkSafetyCheck: View {
    let safety: LinkSafety
    let confirm: () -> Void
    let cancel: () -> Void
    var mismatch: () -> Void = {}
    /// "iPhone" → "your iPhone"; a name someone chose stays as is.
    static func yours(_ name: String) -> String {
        let n = name.trimmingCharacters(in: .whitespaces)
        if n.isEmpty || n == "Your other device" { return "your other device" }
        let first = n.split(separator: " ").first.map { $0.lowercased() } ?? ""
        return ["iphone", "ipad", "mac", "macbook", "pc", "computer", "phone"].contains(first) ? "your \(n)" : n
    }
    private var warning: String {
        "If they match, your friends and chats will be shared between the two devices. Only link devices that are yours."
    }
    var body: some View {
        VStack(spacing: 22) {
            VStack(spacing: 8) {
                Image(systemName: "checkmark.shield.fill").font(.system(size: 46)).foregroundStyle(.tint).accessibilityHidden(true)
                Text("Look at \(LinkSafetyCheck.yours(safety.name)). Does it show these same 6 numbers?").font(.title3.weight(.semibold)).multilineTextAlignment(.center)
                Text("Choose the same answer on both devices.").font(.subheadline).foregroundStyle(.secondary)
            }
            GlassCard {
                Text(safety.safety)
                    .font(.system(size: 46, weight: .semibold, design: .monospaced)).monospacedDigit()
                    .lineLimit(1).minimumScaleFactor(0.5)
                    .frame(maxWidth: .infinity).padding(.vertical, 10)
                    .accessibilityLabel("Safety code \(safety.safety.filter { !$0.isWhitespace }.map(String.init).joined(separator: " "))")
            }
            Label { Text(warning) } icon: { Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange) }
                .font(.subheadline).frame(maxWidth: .infinity, alignment: .leading)
            if !safety.peerShowsCode {
                Text("\(safety.name) needs an update to show the numbers. Only continue if it’s yours.")
                    .font(.subheadline).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading)
            }
            VStack(spacing: 10) {
                Button(action: confirm) { Text("Yes, They Match").frame(maxWidth: .infinity, minHeight: 36) }.beamButton(prominent: true)
                Button(action: mismatch) { Text("No, They’re Different").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                Button(role: .cancel, action: cancel) { Text("Cancel") }.font(.body).padding(.top, 2)
            }
        }
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
