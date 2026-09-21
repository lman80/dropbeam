import SwiftUI
import CoreImage.CIFilterBuiltins

struct SettingsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var version = ""
    @State private var clearCache = false
    @State private var linkNew = false
    @State private var linkThis = false
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    profile
                    SettingsGroup(title: "Devices") {
                        HStack(spacing: 14) {
                            Image(systemName: "iphone").font(.title)
                            VStack(alignment: .leading, spacing: 5) { Text("This iPhone").font(.headline); Text("\(bridge.myDevice?.linkedDevices ?? 0) linked devices").foregroundStyle(.secondary) }
                        }.frame(minHeight: 44)
                        Divider()
                        settingsButton("Link a New Device", symbol: "qrcode.viewfinder") { linkNew = true }
                        Divider()
                        settingsButton("Link This Device", symbol: "link") { linkThis = true }
                    }
                    SettingsGroup(title: "General") {
                        HStack { Text("Appearance"); Spacer(); Picker("Appearance", selection: settingString("theme", bridge.settings?.theme ?? "system")) { Text("System").tag("system"); Text("Light").tag("light"); Text("Dark").tag("dark") }.pickerStyle(.menu).labelsHidden() }.frame(minHeight: 44)
                        Divider(); SettingToggle(title: "Sounds", key: "playSounds", value: bridge.settings?.playSounds)
                        Divider(); SettingToggle(title: "File Notifications", key: "notifyOnComplete", value: bridge.settings?.notifyOnComplete)
                        Divider(); SettingToggle(title: "Chat Notifications", key: "notifyOnMessage", value: bridge.settings?.notifyOnMessage)
                        Divider(); SettingToggle(title: "Read Receipts", key: "sendReadReceipts", value: bridge.settings?.sendReadReceipts)
                        Text("Keep DropBeam open while transferring. Delivery can pause in the background.").font(.footnote).foregroundStyle(.secondary)
                        Divider(); NavigationLink { TextSettingView(title: "Giphy Key", key: "giphyApiKey", value: bridge.settings?.giphyApiKey ?? "", footer: "Add a key from developers.giphy.com to enable GIFs. Leave blank to hide the picker.") } label: { SettingsLinkLabel(title: "Giphy Key", symbol: "photo.on.rectangle") }
                    }
                    transferSettings
                    SettingsGroup(title: "Advanced") {
                        NavigationLink { TextSettingView(title: "Custom Relay", key: "customRelay", value: bridge.settings?.customRelay ?? "", footer: "Use the same relay URL on both devices. Leave blank for public relays. Close and reopen DropBeam to apply changes. Setup: github.com/lman80/dropbeam → RELAY-SETUP.md") } label: { SettingsLinkLabel(title: "Custom Relay", symbol: "network") }
                        Divider(); NavigationLink { DiagnosticsView() } label: { SettingsLinkLabel(title: "Diagnostics", symbol: "waveform.path.ecg") }
                        Divider(); NavigationLink { RecoverySettingsView() } label: { SettingsLinkLabel(title: "Recoverable Files", symbol: "clock.arrow.circlepath") }
                    }
                    SettingsGroup(title: "About") { HStack { Text("Version"); Spacer(); Text(version.isEmpty ? "…" : version).foregroundStyle(.secondary) }.frame(minHeight: 44) }
                    Text("DropBeam · Direct, end-to-end encrypted transfers").font(.footnote).foregroundStyle(.secondary).frame(maxWidth: .infinity)
                }.padding(20)
            }.contentMargins(.bottom, 24, for: .scrollContent).navigationTitle("Settings").navigationBarTitleDisplayMode(.large).beamCanvas()
                .task { do { version = try await bridge.call("appVersion"); try await bridge.action("myDeviceInfo") } catch { bridge.errorMessage = error.localizedDescription } }
                .sheet(isPresented: $linkNew) { QRScannerSheet(title: "Link a New Device") { code in _ = try await bridge.linkDeviceSend(code: code); bridge.showToast("Device linked") } }
                .sheet(isPresented: $linkThis) { LinkThisDeviceSheet() }
                .confirmationDialog("Clear interrupted transfer leftovers?", isPresented: $clearCache, titleVisibility: .visible) { Button("Clear Transfer Cache", role: .destructive) { bridge.perform { let freed: Double = try await bridge.call("clearTransferCache"); bridge.showToast(freed > 0 ? "Cleared \(Formatters.bytes(freed))" : "No transfer leftovers to clear") } } }
        }
    }
    private var profile: some View {
        GlassCard {
            HStack(spacing: 18) {
                Button { bridge.perform { try await bridge.action("setAvatar") } } label: { MyAvatar(size: 72) }.buttonStyle(.plain).accessibilityLabel("Change profile picture")
                NavigationLink { ProfileView() } label: {
                    HStack {
                        VStack(alignment: .leading, spacing: 6) { Text(bridge.settings?.displayName ?? "Your Profile").font(.title2.bold()).foregroundStyle(.primary); Text("DropBeam code").font(.caption).foregroundStyle(.secondary) }
                        Spacer(minLength: 0); Image(systemName: "chevron.right").foregroundStyle(.tertiary)
                    }.frame(minHeight: 72)
                }.buttonStyle(.plain)
            }
        }
    }
    private var transferSettings: some View {
        SettingsGroup(title: "Transfers") {
            SettingToggle(title: "Prefer Direct Connections", key: "preferDirectP2p", value: bridge.settings?.preferDirectP2p)
            Divider(); SettingToggle(title: "Direct Connections Only", key: "requireDirect", value: bridge.settings?.requireDirect)
            Text("Fail the send if a direct path cannot be made. Shared folders use the best available path.").font(.footnote).foregroundStyle(.secondary)
            if bridge.settings?.waitForDirect != nil {
                Divider(); SettingToggle(title: "Wait for a Direct Link", key: "waitForDirect", value: bridge.settings?.requireDirect == true ? false : bridge.settings?.waitForDirect).disabled(bridge.settings?.requireDirect == true)
            }
            Divider(); SettingToggle(title: "Parallel Streams", key: "parallelStreams", value: bridge.settings?.parallelStreams)
            Text("Several connections can speed up files over 16 MB. Turn off if transfers stall.").font(.footnote).foregroundStyle(.secondary)
            Divider(); NavigationLink { UploadLimitView() } label: { SettingsLinkLabel(title: "Upload Limit · \(Int(bridge.settings?.uploadLimitMbps ?? 0)) Mbps", symbol: "speedometer") }
            Divider(); SettingToggle(title: "Speeds in Megabits", key: "showMegabits", value: bridge.settings?.showMegabits)
            Divider(); NavigationLink { ConnectionInfoView() } label: { SettingsLinkLabel(title: "How Transfers Connect", symbol: "antenna.radiowaves.left.and.right") }
            Divider(); Button("Clear Transfer Cache", role: .destructive) { clearCache = true }.frame(minHeight: 44)
        }
    }
    private func settingString(_ key: String, _ value: String) -> Binding<String> { Binding(get: { value }, set: { next in bridge.perform { try await bridge.updateSettings(patch: [key: next]) } }) }
    private func settingsButton(_ title: String, symbol: String, action: @escaping () -> Void) -> some View { Button { Haptics.tap(); action() } label: { SettingsLinkLabel(title: title, symbol: symbol) } }
}
struct SettingsGroup<Content: View>: View {
    let title: String
    @ViewBuilder var content: Content
    var body: some View { VStack(alignment: .leading, spacing: 12) { Text(title).font(.title2.weight(.semibold)); GlassCard { VStack(alignment: .leading, spacing: 14) { content } } } }
}
struct SettingsLinkLabel: View {
    let title: String
    let symbol: String
    var body: some View { HStack(spacing: 12) { Image(systemName: symbol).foregroundStyle(.tint).frame(width: 26); Text(title).foregroundStyle(.primary); Spacer(minLength: 0); Image(systemName: "chevron.right").font(.footnote).foregroundStyle(.tertiary) }.frame(minHeight: 44).contentShape(Rectangle()) }
}
struct SettingToggle: View {
    @EnvironmentObject private var bridge: Bridge
    let title: String
    let key: String
    let value: Bool?
    var body: some View { Toggle(title, isOn: Binding(get: { value ?? false }, set: { next in bridge.perform { try await bridge.updateSettings(patch: [key: next]) } })).frame(minHeight: 44) }
}
struct MyAvatar: View {
    @EnvironmentObject private var bridge: Bridge
    let size: CGFloat
    var body: some View { FriendAvatar(friend: Friend(id: "self", name: bridge.settings?.displayName ?? "Me", avatar: bridge.settings?.avatar), size: size) }
}
struct ProfileView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var code = ""
    @State private var name = ""
    @State private var editing = false
    var body: some View {
        ScrollView {
            VStack(spacing: 24) {
                Button { bridge.perform { try await bridge.action("setAvatar") } } label: { MyAvatar(size: 112) }.buttonStyle(.plain).accessibilityLabel("Change profile picture")
                    .contextMenu { Button("Remove Picture", role: .destructive) { bridge.perform { try await bridge.action("clearAvatar") } } }
                Button { name = bridge.settings?.displayName ?? ""; editing = true } label: { HStack { Text(bridge.settings?.displayName ?? "Your Name").font(.title2.bold()); Image(systemName: "pencil").font(.body) } }.frame(minHeight: 44)
                GlassCard {
                    VStack(spacing: 20) {
                        Text("Your DropBeam code").font(.headline)
                        if code.isEmpty { ProgressView() } else { InviteQRCode(code: code); Text(code).font(.caption.monospaced()).textSelection(.enabled) }
                        ViewThatFits(in: .horizontal) { HStack(spacing: 12) { codeButtons }; VStack(spacing: 12) { codeButtons } }
                    }.frame(maxWidth: .infinity)
                }
            }.padding(24)
        }.navigationTitle("Profile").navigationBarTitleDisplayMode(.inline).beamCanvas()
            .task { bridge.perform { code = try await bridge.myInviteCode() } }
            .alert("Display Name", isPresented: $editing) { TextField("Name", text: $name); Button("Cancel", role: .cancel) {}; Button("Save") { bridge.perform { try await bridge.action("setDisplayName", ["name": name]) } }.disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty) }
    }
    @ViewBuilder private var codeButtons: some View {
        Button { UIPasteboard.general.string = code; Haptics.tap(); bridge.showToast("Code copied") } label: { Label("Copy Code", systemImage: "doc.on.doc").frame(minHeight: 44) }.beamButton().disabled(code.isEmpty)
        ShareLink(item: code) { Label("Share Code", systemImage: "square.and.arrow.up").frame(minHeight: 44) }.beamButton(prominent: true).disabled(code.isEmpty)
    }
}
struct InviteQRCode: View {
    let code: String
    private var image: UIImage? {
        let filter = CIFilter.qrCodeGenerator(); filter.message = Data(code.utf8); filter.correctionLevel = "M"
        guard let output = filter.outputImage, let cg = CIContext().createCGImage(output.transformed(by: CGAffineTransform(scaleX: 8, y: 8)), from: output.extent.applying(CGAffineTransform(scaleX: 8, y: 8))) else { return nil }
        return UIImage(cgImage: cg)
    }
    var body: some View {
        Group { if let image { Image(uiImage: image).interpolation(.none).resizable().scaledToFit() } else { Image(systemName: "qrcode").resizable().scaledToFit() } }
            .padding(20).frame(maxWidth: 280).background(.white, in: RoundedRectangle(cornerRadius: 16)).accessibilityLabel("DropBeam QR code")
    }
}
struct LinkThisDeviceSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    @State private var code = ""
    @State private var error: String?
    @State private var baseline = Set<String>()
    @State private var watching = false
    @State private var closed = false
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 24) {
                    BeamEmpty(symbol: "link", title: "Bring your devices together.", detail: "On your other device, choose Link a New Device and scan this code.")
                    GlassCard {
                        VStack(spacing: 18) {
                            if code.isEmpty && error == nil { ProgressView() }
                            if !code.isEmpty { InviteQRCode(code: code); Text(code).font(.caption.monospaced()).textSelection(.enabled); Button("Copy Code") { UIPasteboard.general.string = code; Haptics.tap() }.beamButton() }
                            if let error { Text(error).foregroundStyle(.red); Button("Try Again") { Task { await begin() } }.beamButton() }
                        }.frame(maxWidth: .infinity)
                    }
                }.padding(20)
            }.navigationTitle("Link This Device").navigationBarTitleDisplayMode(.inline).beamCanvas()
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
                .task { await begin() }
                .onChange(of: linkedIDs) { _, ids in
                    if watching && !ids.subtracting(baseline).isEmpty { bridge.showToast("Device linked successfully"); dismiss() }
                }
                .onDisappear { closed = true; Task { try? await bridge.linkDeviceCancel() } }
        }
    }
    private var linkedIDs: Set<String> {
        guard let account = bridge.myDevice?.accountPub, !account.isEmpty else { return [] }
        return Set(bridge.friends.filter { $0.accountPub == account }.map(\.id))
    }
    private func begin() async {
        error = nil
        do {
            if !watching {
                try await bridge.action("myDeviceInfo")
                guard !closed else { return }
                baseline = linkedIDs; watching = true
            }
            let next = try await bridge.linkDeviceBegin()
            if closed { try? await bridge.linkDeviceCancel() } else { code = next }
        }
        catch { self.error = error.localizedDescription }
    }
}
struct TextSettingView: View {
    @EnvironmentObject private var bridge: Bridge
    let title: String
    let key: String
    let value: String
    let footer: String
    @State private var draft = ""
    @State private var saving = false
    var body: some View {
        ScrollView { GlassCard { VStack(alignment: .leading, spacing: 20) {
            TextField(title, text: $draft, axis: .vertical).textInputAutocapitalization(.never).autocorrectionDisabled().padding(14).background(.quaternary, in: RoundedRectangle(cornerRadius: 14))
            Text(footer).font(.subheadline).foregroundStyle(.secondary)
            Button(saving ? "Saving…" : "Save") { saving = true; bridge.perform { defer { saving = false }; try await bridge.updateSettings(patch: [key: draft.trimmingCharacters(in: .whitespacesAndNewlines)]); bridge.showToast("Saved") } }.beamButton(prominent: true).disabled(saving)
        } }.padding(20) }.navigationTitle(title).navigationBarTitleDisplayMode(.inline).beamCanvas().onAppear { draft = value }
    }
}
struct UploadLimitView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var limit = ""
    var body: some View {
        ScrollView { GlassCard { VStack(alignment: .leading, spacing: 18) {
            TextField("Mbps", text: $limit).keyboardType(.numberPad).font(.title2).frame(minHeight: 44)
            Text("0 means unlimited. Local transfers run at full speed. Start at 100 Mbps and adjust if your Wi-Fi stutters.").foregroundStyle(.secondary)
            Button("Save") { bridge.perform { try await bridge.updateSettings(patch: ["uploadLimitMbps": min(100000, max(0, Int(limit) ?? 0))]); bridge.showToast("Upload limit saved") } }.beamButton(prominent: true).disabled(Int(limit) == nil)
        } }.padding(20) }.navigationTitle("Upload Limit").beamCanvas().onAppear { limit = String(Int(bridge.settings?.uploadLimitMbps ?? 0)) }
    }
}
struct DiagnosticsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var busy = false
    @State private var result: String?
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                SettingsGroup(title: "Diagnostics") {
                    SettingToggle(title: "Detailed Logging", key: "verboseLogging", value: bridge.settings?.verboseLogging)
                    Text("Extra network logs for reproducing issues. Close and reopen DropBeam to apply.").font(.footnote).foregroundStyle(.secondary)
                    Divider(); SettingToggle(title: "Share Background Diagnostics", key: "shareDiagnostics", value: bridge.settings?.shareDiagnostics)
                    Text("Sends a redacted error and performance summary about once a day. Never includes file names or contents.").font(.footnote).foregroundStyle(.secondary)
                    if bridge.settings?.shareDiagnostics == true {
                        Divider(); NavigationLink { TextSettingView(title: "Diagnostics Endpoint", key: "diagnosticsUrl", value: bridge.settings?.diagnosticsUrl ?? "", footer: "Leave blank for the built-in collector. Override only if you run your own. Use an https:// URL.") } label: { SettingsLinkLabel(title: "Diagnostics Endpoint", symbol: "network") }
                        Button("Send Test") { test() }.beamButton().disabled(busy || invalidEndpoint)
                    }
                    Divider(); Button("Export Logs") { busy = true; bridge.perform { defer { busy = false }; try await bridge.action("exportLogs") } }.beamButton().disabled(busy)
                    if let result { Text(result).font(.subheadline).textSelection(.enabled) }
                }
                SettingsGroup(title: "Lab Mode") {
                    SettingToggle(title: "Enable Lab Mode", key: "labModeEnabled", value: bridge.settings?.labModeEnabled)
                    Text("Allow one trusted developer device to run encrypted diagnostics. Only the operator ID below is accepted. Enable only when asked by the developer.").font(.footnote).foregroundStyle(.secondary)
                    if bridge.settings?.labModeEnabled == true {
                        Divider(); NavigationLink { TextSettingView(title: "Operator ID", key: "labOperatorId", value: bridge.settings?.labOperatorId ?? "", footer: "Only this device can run Lab Mode. Blank accepts no device.") } label: { SettingsLinkLabel(title: "Operator ID", symbol: "person.crop.circle.badge.checkmark") }
                        Button("Copy This Device’s ID") { UIPasteboard.general.string = bridge.myDevice?.endpointId; Haptics.tap(); bridge.showToast("Device ID copied") }.beamButton().disabled(bridge.myDevice?.endpointId == nil)
                    }
                }
            }.padding(20)
        }.navigationTitle("Diagnostics").beamCanvas()
    }
    private var invalidEndpoint: Bool { let url = bridge.settings?.diagnosticsUrl ?? ""; return !url.isEmpty && !url.hasPrefix("https://") }
    private func test() { busy = true; bridge.perform { defer { busy = false }; result = try await bridge.call("diagnosticsTest") } }
}
struct ConnectionInfoView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var busy = false
    @State private var result: String?
    var body: some View {
        ScrollView { VStack(spacing: 24) {
            SettingsGroup(title: "Direct peer-to-peer") {
                Text("End-to-end encrypted. Keep both devices open until transfers finish.").foregroundStyle(.secondary)
                Button(busy ? "Testing…" : "Test Connection") { busy = true; bridge.perform { defer { busy = false }; result = try await bridge.call("connectionTest") } }.beamButton().disabled(busy)
                if let result { Text(result).font(.subheadline).textSelection(.enabled) }
            }
            SettingsGroup(title: "How transfers connect") {
                Text("Local").font(.headline); Text("Same Wi-Fi or network. Files travel directly across your network, without the internet.")
                Divider(); Text("Direct").font(.headline); Text("An encrypted peer-to-peer link across the internet connects both devices.")
                Divider(); Text("Relay").font(.headline); Text("If a direct path is unavailable, an encrypted relay carries files. It cannot read them, but may be slower.")
                Divider(); Text("Connecting").font(.headline); Text("Finding the best available route to the other device.")
            }
            SettingsGroup(title: "Local network access") { Text("If nearby transfers use the relay, enable DropBeam in iOS Settings → Privacy & Security → Local Network on both devices.").foregroundStyle(.secondary) }
        }.padding(20) }.navigationTitle("Connections").beamCanvas()
    }
}
struct RecoverySettingsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var clearing = false
    var body: some View {
        ScrollView { SettingsGroup(title: "Saved copies") {
            Picker("Keep copies for", selection: Binding(get: { bridge.settings?.folderHistoryKeepDays ?? 30 }, set: { n in bridge.perform { try await bridge.updateSettings(patch: ["folderHistoryKeepDays": n]) } })) { Text("7 days").tag(7); Text("30 days").tag(30); Text("90 days").tag(90); Text("Forever").tag(0) }.pickerStyle(.menu)
            Divider()
            Picker("Storage per folder", selection: Binding(get: { bridge.settings?.folderHistoryBudgetBytes ?? 2147483648 }, set: { n in bridge.perform { try await bridge.updateSettings(patch: ["folderHistoryBudgetBytes": n]) } })) { Text("500 MB").tag(524288000.0); Text("2 GB").tag(2147483648.0); Text("5 GB").tag(5368709120.0); Text("No limit").tag(0.0) }.pickerStyle(.menu)
            Text("Deleted and replaced files are kept here until these limits remove the oldest copies. Live files are untouched.").font(.subheadline).foregroundStyle(.secondary)
            Button("Free Up Space Now", role: .destructive) { clearing = true }.frame(minHeight: 44)
        }.padding(20) }.navigationTitle("Recoverable Files").beamCanvas()
            .confirmationDialog("Permanently delete all saved copies?", isPresented: $clearing, titleVisibility: .visible) { Button("Delete Saved Copies", role: .destructive) { bridge.perform { let freed: Double = try await bridge.call("recoverableEmptyAll"); bridge.showToast("Freed \(Formatters.bytes(freed))") } } }
    }
}
