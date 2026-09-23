import SwiftUI

/// Settings → Devices: every device in this account, kept in sync peer to peer.
struct DevicesView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var adding = false
    @State private var joining = false
    @State private var showingCode = false
    @State private var removing: AccountDevice?
    @State private var leaving = false
    @State private var syncing = false
    private var devices: [AccountDevice] { bridge.myDevice?.devices ?? [] }
    private var inAccount: Bool { bridge.myDevice?.inAccount == true && devices.count > 1 }
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
                        }
                        Button { adding = true; Haptics.tap() } label: { Label("Add a Device", systemImage: "plus").frame(maxWidth: .infinity, minHeight: 36) }.beamButton(prominent: true)
                        Button { syncNow() } label: { Label(syncing ? "Syncing…" : "Sync Now", systemImage: "arrow.triangle.2.circlepath").frame(maxWidth: .infinity, minHeight: 36) }.beamButton().disabled(syncing)
                        Button(role: .destructive) { leaving = true } label: { Text("Remove This \(deviceNoun(bridge.myDevice?.deviceKind, os: bridge.myDevice?.deviceOs ?? "ios")) from Account").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                    } else {
                        GlassCard {
                            VStack(alignment: .leading, spacing: 14) {
                                Label("Already use DropBeam?", systemImage: "laptopcomputer.and.iphone").font(.headline)
                                Text("On your other device, open Settings → Devices → Add a Device, then scan the code it shows.").foregroundStyle(.secondary)
                                Button { joining = true; Haptics.tap() } label: { Label("Scan Code", systemImage: "qrcode.viewfinder").frame(maxWidth: .infinity, minHeight: 36) }.beamButton(prominent: true)
                                Button { showingCode = true; Haptics.tap() } label: { Text("Show a Code Instead").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                            }
                        }
                        GlassCard {
                            VStack(alignment: .leading, spacing: 14) {
                                Label("New device?", systemImage: "plus.circle").font(.headline)
                                Text("Bring a new phone or computer into your account. It gets your friends and chats right away.").foregroundStyle(.secondary)
                                Button { adding = true; Haptics.tap() } label: { Label("Add a Device", systemImage: "plus").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                            }
                        }
                    }
                    Text("Your friends and conversations sync directly between your devices — end-to-end encrypted, never stored on a server.")
                        .font(.footnote).foregroundStyle(.secondary)
                }.padding(20)
            }
        }
        .contentMargins(.bottom, 24, for: .scrollContent)
        .navigationTitle("Devices").navigationBarTitleDisplayMode(.large).beamCanvas()
        .refreshable { await refresh(sync: true) }
        .task { await refresh(sync: false) }
        .sheet(isPresented: $adding) { AddDeviceSheet() }
        .sheet(isPresented: $joining) { JoinAccountSheet() }
        .sheet(isPresented: $showingCode) { LinkThisDeviceSheet() }
        .confirmationDialog(removing.map { "Remove \($0.name)?" } ?? "", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible, presenting: removing) { device in
            Button("Remove from Account", role: .destructive) { bridge.perform { try await bridge.accountRemoveDevice(endpointId: device.endpointId); bridge.showToast("\(device.name) removed") } }
        } message: { _ in Text("It stops syncing your friends and chats. You can link it again later.") }
        .confirmationDialog("Remove this device from your account?", isPresented: $leaving, titleVisibility: .visible) {
            Button("Remove", role: .destructive) { bridge.perform { try await bridge.accountLeave(); bridge.showToast("This device left your account") } }
        } message: { Text("Friends and chats stay on this device, but stop syncing with your other devices.") }
    }
    private var hero: some View {
        HStack(spacing: 14) {
            ForEach(Array(devices.prefix(3).enumerated()), id: \.element.id) { _, d in
                DeviceAvatar(kind: d.deviceKind, os: d.deviceOs, size: 56)
            }
            if devices.isEmpty { DeviceAvatar(kind: "phone", os: "ios", size: 56) }
        }.frame(maxWidth: .infinity).padding(.top, 4)
    }
    @ViewBuilder private func row(_ device: AccountDevice) -> some View {
        HStack(spacing: 14) {
            DeviceAvatar(kind: device.deviceKind, os: device.deviceOs, size: 46)
            VStack(alignment: .leading, spacing: 3) {
                Text(device.thisDevice ? "This \(deviceNoun(device.deviceKind, os: device.deviceOs))" : "Your \(deviceNoun(device.deviceKind, os: device.deviceOs))").font(.headline)
                Text(subtitle(device)).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: 4)
            if !device.thisDevice {
                Menu {
                    Button("Remove from Account", systemImage: "minus.circle", role: .destructive) { removing = device }
                } label: { Image(systemName: "ellipsis").frame(width: 36, height: 36).contentShape(Rectangle()) }
                    .accessibilityLabel("Options for \(device.name)")
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
        }
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

/// On the device that HAS the account: show a code the new device scans.
struct AddDeviceSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    @State private var code = ""
    @State private var error: String?
    @State private var scanning = false
    @State private var linked: String?
    @State private var closed = false
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 22) {
                    if let linked {
                        BeamEmpty(symbol: "checkmark.circle.fill", title: "\(linked) is linked", detail: "Your friends and chats are on it now, and they'll stay in sync.")
                        Button("Done") { dismiss() }.beamButton(prominent: true)
                    } else {
                        Text("On your new device, open DropBeam and choose **Already use DropBeam?** — or Settings → Devices → Scan Code — then scan this code.")
                            .multilineTextAlignment(.center).foregroundStyle(.secondary)
                        GlassCard {
                            VStack(spacing: 16) {
                                if code.isEmpty && error == nil { ProgressView().frame(height: 240) }
                                if !code.isEmpty { InviteQRCode(code: code) }
                                if let error { Text(error).foregroundStyle(.red); Button("Try Again") { Task { await begin() } }.beamButton() }
                                Label("Waiting for your other device…", systemImage: "antenna.radiowaves.left.and.right").font(.subheadline).foregroundStyle(.secondary).opacity(code.isEmpty ? 0 : 1)
                            }.frame(maxWidth: .infinity)
                        }
                        Button { scanning = true; Haptics.tap() } label: { Label("Scan the Other Device Instead", systemImage: "qrcode.viewfinder").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                        if !code.isEmpty { Button("Copy Code") { UIPasteboard.general.string = code; Haptics.tap(); bridge.showToast("Code copied") }.font(.subheadline) }
                    }
                }.padding(20)
            }
            .navigationTitle("Add a Device").navigationBarTitleDisplayMode(.inline).beamCanvas()
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button(linked == nil ? "Cancel" : "Close") { dismiss() } } }
            .task { await begin() }
            .onReceive(NotificationCenter.default.publisher(for: Notification.Name("DropBeam.link://linked"))) { note in
                let payload = note.userInfo?["payload"] as? [String: Any]
                withAnimation(.smooth) { linked = (payload?["name"] as? String) ?? "Your device" }
                UINotificationFeedbackGenerator().notificationOccurred(.success)
            }
            .onReceive(NotificationCenter.default.publisher(for: Notification.Name("DropBeam.link://failed"))) { note in
                error = (note.userInfo?["payload"] as? String) ?? "The other device couldn't join."
            }
            .sheet(isPresented: $scanning) {
                QRScannerSheet(title: "Scan Other Device", autoSubmit: true) { value in
                    let result = try await bridge.linkWithScannedCode(value)
                    withAnimation(.smooth) { linked = result.name ?? "Your device" }
                }
            }
            .onDisappear { closed = true; Task { try? await bridge.linkHostCancel() } }
        }
    }
    private func begin() async {
        error = nil
        do {
            let next = try await bridge.linkHostBegin()
            if closed { try? await bridge.linkHostCancel() } else { code = next }
        } catch { self.error = error.localizedDescription }
    }
}

/// On a NEW device: scan the code shown by the device that has the account.
struct JoinAccountSheet: View {
    @EnvironmentObject private var bridge: Bridge
    var body: some View {
        QRScannerSheet(title: "Link to Your Account", autoSubmit: true) { code in
            let result = try await bridge.linkWithScannedCode(code)
            UINotificationFeedbackGenerator().notificationOccurred(.success)
            bridge.showToast("Linked with \(result.name ?? "your device") — syncing your friends and chats")
        }
    }
}
