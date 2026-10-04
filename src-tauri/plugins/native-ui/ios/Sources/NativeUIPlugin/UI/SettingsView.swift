import SwiftUI
import CoreImage.CIFilterBuiltins

enum DropBeamLinks {
    static let privacy = URL(string: "https://github.com/lman80/dropbeam/blob/main/PRIVACY.md")!
    static let support = URL(string: "https://github.com/lman80/dropbeam/issues")!
    static let relaySetup = URL(string: "https://github.com/lman80/dropbeam/blob/main/RELAY-SETUP.md")!
}

struct SettingsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var version = ""
    @State private var clearCache = false
    @State private var feedbackButton = SuperFeedback.isEnabled
    @ObservedObject private var saveFolder = SaveFolder.shared
    @ObservedObject private var mediaSaver = ReceivedMediaSaver.shared
    @State private var qaSaveFolder = false
    @State private var qaPrivacy = false
    @State private var pendingStrings: [String: String] = [:]
    var body: some View {
        NavigationStack {
          ScrollViewReader { proxy in
            List {
                Section {
                    NavigationLink { ProfileView() } label: {
                        HStack(spacing: 16) {
                            MyAvatar(size: 62)
                            VStack(alignment: .leading, spacing: 3) {
                                Text(bridge.settings?.displayName ?? "Your Profile").font(.title3.weight(.semibold)).foregroundStyle(.primary).lineLimit(1)
                                    .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                                Text("Name, photo & DropBeam code").font(.subheadline).foregroundStyle(.secondary)
                            }
                        }.padding(.vertical, 4)
                    }
                    NavigationLink { DevicesView() } label: {
                        HStack(spacing: 14) {
                            RowIcon(symbol: "laptopcomputer.and.iphone", color: .gray)
                            VStack(alignment: .leading, spacing: 2) {
                                Text("My Devices").foregroundStyle(.primary).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                                Text(devicesSummary).font(.subheadline).foregroundStyle(.secondary).lineLimit(2)
                            }
                        }
                    }
                }
                Section {
                    NavigationLink { SaveFolderView() } label: {
                        RowLabel(title: "Save Files To", symbol: "folder.fill", color: .blue, value: saveFolder.displayName)
                    }
                    IconToggle(title: "Save Photos & Videos", symbol: "photo.fill.on.rectangle.fill", color: .orange,
                               isOn: Binding(get: { mediaSaver.choice == .on }, set: { on in Task { await mediaSaver.setEnabled(on) } }))
                    NavigationLink { TransferSettingsView() } label: { RowLabel(title: "Connection & Speed", symbol: "arrow.up.arrow.down", color: .teal) }
                } header: { Text("Transfers") } footer: {
                    Text("Received photos and videos are also added to your photo library. The originals stay in your save folder.")
                }
                Section("General") {
                    Picker(selection: settingString("theme", bridge.settings?.theme ?? "system")) {
                        Text("System").tag("system"); Text("Light").tag("light"); Text("Dark").tag("dark")
                    } label: { RowLabel(title: "Appearance", symbol: "circle.lefthalf.filled", color: .indigo) }
                    SettingToggle(title: "Sounds", symbol: "speaker.wave.2.fill", color: .pink, key: "playSounds", value: bridge.settings?.playSounds)
                }
                Section {
                    SettingToggle(title: "Files", symbol: "bell.badge.fill", color: .red, key: "notifyOnComplete", value: bridge.settings?.notifyOnComplete)
                    SettingToggle(title: "Messages", symbol: "message.fill", color: .green, key: "notifyOnMessage", value: bridge.settings?.notifyOnMessage)
                } header: { Text("Notifications") } footer: { Text("Keep DropBeam open while transferring. iOS can pause transfers in the background.") }
                Section {
                    SettingToggle(title: "Read Receipts", symbol: "checkmark.message.fill", color: .blue, key: "sendReadReceipts", value: bridge.settings?.sendReadReceipts)
                    SettingToggle(title: "Link Previews", symbol: "link", color: .teal, key: "linkPreviews", value: bridge.settings?.linkPreviews ?? true)
                    NavigationLink { TextSettingView(title: "GIF Search", key: "giphyApiKey", value: bridge.settings?.giphyApiKey ?? "", placeholder: "Giphy API key", footer: "Add a free key from developers.giphy.com to search GIFs in chats. Leave blank to hide the GIF button.") } label: {
                        RowLabel(title: "GIF Search", symbol: "sparkles.rectangle.stack.fill", color: .purple, value: (bridge.settings?.giphyApiKey ?? "").isEmpty ? "Off" : "On")
                    }
                } header: { Text("Chat") } footer: { Text("Friends see when you’ve read their messages while Read Receipts is on. With Link Previews, this iPhone fetches a small preview of a link you send; friends never contact the site.") }
                TransferServersSection()
                Section("Storage") {
                    NavigationLink { RecoverySettingsView() } label: { RowLabel(title: "Recoverable Files", symbol: "clock.arrow.circlepath", color: .teal) }
                    ActionRow(title: "Clear Transfer Cache", symbol: "trash.fill", color: .gray) { clearCache = true }
                }
                Section {
                    NavigationLink { PrivacyView() } label: { RowLabel(title: "Privacy & Your Data", symbol: "hand.raised.fill", color: .blue) }
                    NavigationLink { BlockedView() } label: { RowLabel(title: "Blocked", symbol: "nosign", color: .gray, value: bridge.blocked.isEmpty ? nil : "\(bridge.blocked.count)") }
                    NavigationLink { DiagnosticsView() } label: { RowLabel(title: "Diagnostics", symbol: "waveform.path.ecg", color: .red) }
                    LinkRow(title: "Help & Support", symbol: "questionmark.circle.fill", color: .green, url: DropBeamLinks.support)
                    ActionRow(title: "Report a Problem", symbol: "envelope.fill", color: .orange) { contact() }
                } header: { Text("Privacy & Support") }
                Section {
                    IconToggle(title: "Show Feedback Button", symbol: "hand.tap.fill", color: .gray, isOn: Binding(get: { feedbackButton }, set: { feedbackButton = $0; SuperFeedback.setEnabled($0) }))
                        .id("feedbackSection")
                    ActionRow(title: "Send Feedback", symbol: "bubble.left.and.bubble.right.fill", color: .beam) { SuperFeedback.present() }
                    ActionRow(title: "Ideas & Roadmap", symbol: "lightbulb.fill", color: .yellow) { SuperFeedback.present(tab: .ideas) }
                    // Hidden until DropBeam has a way to take support here (none on iOS yet).
                    SuperFeedbackSupportRow { RowLabel(title: "Support Development", symbol: "heart.fill", color: .pink).contentShape(Rectangle()) }
                        .buttonStyle(.plain)
                } header: { Text("Feedback") } footer: {
                    Text("Feedback goes straight to the developer, and you can vote on what gets built next in Ideas & Roadmap. DropBeam is built with AI, so a good suggestion can ship in an update within days.")
                }
                Section {
                    LabeledContent("Version", value: version.isEmpty ? "…" : version)
                } footer: {
                    Text("End-to-end encrypted").frame(maxWidth: .infinity).padding(.top, 8)
                }
            }
            .beamList()
            .navigationTitle("Settings")
            .navigationDestination(isPresented: $qaSaveFolder) { SaveFolderView() }
            .navigationDestination(isPresented: $qaPrivacy) { PrivacyView() }
            #if targetEnvironment(simulator)
            .onAppear { if CommandLine.arguments.contains("-openSaveFolder") { qaSaveFolder = true } }
            // QA: `-openPrivacy` opens Privacy & Your Data (Erase All Data).
            .onAppear { if CommandLine.arguments.contains("-openPrivacy") { qaPrivacy = true } }
            // QA: `-scrollToServers` brings the Transfer Servers section into view.
            // QA: `-scrollToFeedback` brings the Feedback section into view.
            .task { if CommandLine.arguments.contains("-scrollToFeedback") { try? await Task.sleep(for: .seconds(1)); proxy.scrollTo("feedbackSection", anchor: .top) } }
            .task { if CommandLine.arguments.contains("-scrollToServers") { try? await Task.sleep(for: .seconds(1)); if CommandLine.arguments.contains("-scrollToServerPush") { proxy.scrollTo("serverNotifications", anchor: .center) } else { proxy.scrollTo("transferServersEnd", anchor: .bottom) } } }
            #endif
            .task { version = (try? await bridge.call("appVersion")) ?? ""; try? await bridge.action("myDeviceInfo") }
            .onAppear { feedbackButton = SuperFeedback.isEnabled }
            .confirmationDialog("Clear interrupted transfer leftovers?", isPresented: $clearCache, titleVisibility: .visible) {
                Button("Clear Transfer Cache", role: .destructive) { bridge.perform { let freed: Double = try await bridge.call("clearTransferCache"); bridge.showToast(freed > 0 ? "Cleared \(Formatters.bytes(freed))" : "No transfer leftovers to clear") } }
            } message: { Text("Partly received files are removed. Finished files aren’t affected.") }
          }
        }
    }
    /// Email the DropBeam team (the address lives in src/lib/report.ts).
    private func contact() {
        bridge.perform {
            let mail = try await bridge.call("contactMail") as ReportMail
            if let url = URL(string: mail.url), await UIApplication.shared.open(url) { return }
            UIPasteboard.general.string = mail.to
            bridge.showToast("No mail app set up — address copied: \(mail.to)")
        }
    }
    private var devicesSummary: String {
        let others = (bridge.myDevice?.devices ?? []).filter { !$0.thisDevice }
        if others.isEmpty { return "Link your other devices to share friends and chats" }
        return "This iPhone and " + ListFormatter.localizedString(byJoining: others.map { "your " + deviceNoun($0.deviceKind, os: $0.deviceOs) })
    }
    private func settingString(_ key: String, _ value: String) -> Binding<String> {
        Binding(get: { pendingStrings[key] ?? value }, set: { next in
            pendingStrings[key] = next
            bridge.perform { await OptimisticSetting.save(key: key, value: next, bridge: bridge) { pendingStrings[key] = nil } }
        })
    }
}
/// A settings toggle row that writes straight through to the engine. It flips at once
/// (optimistic) and only snaps back if the engine refuses — no flicker while the
/// round trip through the bridge and the next settings snapshot completes.
struct SettingToggle: View {
    @EnvironmentObject private var bridge: Bridge
    let title: String
    var symbol: String? = nil
    var color: Color = .beam
    let key: String
    let value: Bool?
    @State private var pending: Bool?
    var body: some View {
        let binding = Binding(get: { pending ?? value ?? false }, set: { next in
            pending = next
            bridge.perform { await OptimisticSetting.save(key: key, value: next, bridge: bridge) { pending = nil } }
        })
        Group {
            if let symbol { IconToggle(title: title, symbol: symbol, color: color, isOn: binding) }
            else { Toggle(title, isOn: binding).tint(.green) }
        }
        .onChange(of: value) { _, now in if now == pending { pending = nil } }
    }
}
/// Save one setting; `settle` runs when the optimistic value should give way to the
/// engine's (refused right away, or the snapshot had time to arrive).
enum OptimisticSetting {
    @MainActor static func save(key: String, value: Any, bridge: Bridge, settle: @escaping @MainActor () -> Void) async {
        do {
            try await bridge.updateSettings(patch: [key: value])
            try? await Task.sleep(for: .seconds(3))
        } catch {
            bridge.errorMessage = error.localizedDescription
        }
        settle()
    }
}
struct MyAvatar: View {
    @EnvironmentObject private var bridge: Bridge
    let size: CGFloat
    // Desktop colours your own monogram by your name (avatarColor(name || 'you')).
    var body: some View {
        let name = bridge.settings?.displayName ?? ""
        FriendAvatar(friend: Friend(id: "self", name: name.isEmpty ? "Me" : name, avatar: bridge.settings?.avatar), size: size, colorSeed: name.isEmpty ? "you" : name)
    }
}
struct ProfileView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var name = ""
    @State private var editing = false
    var body: some View {
        List {
            Section {
                VStack(spacing: 12) {
                    Menu {
                        Button("Choose Photo", systemImage: "photo") { bridge.perform { try await bridge.pickAvatar() } }
                        if bridge.settings?.avatar != nil { Button("Remove Photo", systemImage: "trash", role: .destructive) { bridge.perform { try await bridge.action("clearAvatar") } } }
                    } label: {
                        MyAvatar(size: 116).overlay(alignment: .bottomTrailing) {
                            Image(systemName: "camera.fill").font(.footnote.weight(.semibold)).foregroundStyle(.white)
                                .frame(width: 32, height: 32).background(Color.beam, in: Circle())
                                .overlay(Circle().stroke(Color(uiColor: .systemGroupedBackground), lineWidth: 3))
                        }
                    }.accessibilityLabel("Change profile photo")
                    Text(bridge.settings?.displayName ?? "Your Name").font(.title2.bold()).multilineTextAlignment(.center).lineLimit(3)
                    Text("Friends see this name and photo.").font(.subheadline).foregroundStyle(.secondary)
                }.frame(maxWidth: .infinity)
            }.clearRow()
            MyCodeSection()
        }
        .beamList()
        .navigationTitle("Profile").navigationBarTitleDisplayMode(.inline)
        .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Edit") { name = bridge.settings?.displayName ?? ""; editing = true }.accessibilityLabel("Edit name") } }
        .alert("Display Name", isPresented: $editing) {
            TextField("Name", text: $name).textContentType(.name)
            Button("Cancel", role: .cancel) {}
            Button("Save") { bridge.perform { try await bridge.action("setDisplayName", ["name": name]) } }.disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        } message: { Text("Friends see this name when you send files and chat.") }
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
            .padding(14).frame(maxWidth: 220).background(.white, in: RoundedRectangle(cornerRadius: 18, style: .continuous))
            .accessibilityLabel("DropBeam QR code")
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
            List {
                Section {
                    VStack(spacing: 10) {
                        Image(systemName: "link.circle.fill").font(.system(size: 48)).foregroundStyle(.tint).accessibilityHidden(true)
                        Text("Bring your devices together.").font(.title3.weight(.semibold))
                        Text("On your other device, choose Link a New Device and scan this code.").font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
                    }.frame(maxWidth: .infinity)
                }.clearRow()
                Section {
                    VStack(spacing: 16) {
                        if code.isEmpty && error == nil { ProgressView().frame(height: 240) }
                        if !code.isEmpty {
                            InviteQRCode(code: code)
                            CodeLine(code: code).padding(.horizontal, 12)
                            Button { UIPasteboard.general.string = code; Haptics.success() } label: { Label("Copy Code", systemImage: "doc.on.doc") }.beamButton()
                        }
                        if let error {
                            Text(error).foregroundStyle(.red).multilineTextAlignment(.center)
                            Button("Try Again") { Task { await begin() } }.beamButton(prominent: true)
                        }
                    }.frame(maxWidth: .infinity).padding(.vertical, 12)
                }
            }
            .beamList()
            .navigationTitle("Link This Device").navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
            .task { await begin() }
            .onChange(of: linkedIDs) { _, ids in
                if watching && !ids.subtracting(baseline).isEmpty { Haptics.success(); bridge.showToast("Device linked successfully"); dismiss() }
            }
            .onDisappear { closed = true; Task { try? await bridge.linkDeviceCancel() } }
        }.tint(.beam)
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
    @Environment(\.dismiss) private var dismiss
    let title: String
    let key: String
    let value: String
    var placeholder = ""
    let footer: String
    var link: (String, URL)? = nil
    @State private var draft = ""
    @State private var saving = false
    private var changed: Bool { draft.trimmingCharacters(in: .whitespacesAndNewlines) != value }
    var body: some View {
        Form {
            Section {
                TextField(placeholder.isEmpty ? title : placeholder, text: $draft, axis: .vertical)
                    .textInputAutocapitalization(.never).autocorrectionDisabled().font(.body.monospaced()).submitLabel(.done)
                if !draft.isEmpty { Button("Clear", role: .destructive) { draft = "" } }
            } footer: {
                VStack(alignment: .leading, spacing: 6) {
                    Text(footer)
                    if let link { Link(link.0, destination: link.1) }
                }
            }
        }
        .scrollContentBackground(.hidden).background { BeamBackground() }
        .navigationTitle(title).navigationBarTitleDisplayMode(.inline)
        .toolbar { ToolbarItem(placement: .confirmationAction) {
            Button(saving ? "Saving…" : "Save") {
                saving = true
                bridge.perform { defer { saving = false }; try await bridge.updateSettings(patch: [key: draft.trimmingCharacters(in: .whitespacesAndNewlines)]); Haptics.success(); bridge.showToast("Saved"); dismiss() }
            }.disabled(saving || !changed)
        } }
        .onAppear { draft = value }
    }
}
struct UploadLimitView: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    @State private var limit = ""
    private let presets = [0, 10, 25, 50, 100, 250]
    private var current: Int { Int(bridge.settings?.uploadLimitMbps ?? 0) }
    var body: some View {
        Form {
            Section {
                ForEach(presets, id: \.self) { value in
                    Button { save(value) } label: {
                        HStack { Text(value == 0 ? "No Limit" : "\(value) Mbps").foregroundStyle(.primary); Spacer(); if current == value { Image(systemName: "checkmark").foregroundStyle(.tint).fontWeight(.semibold) } }
                    }.accessibilityAddTraits(current == value ? .isSelected : [])
                }
            } footer: { Text("Local transfers always run at full speed. If your Wi-Fi stutters while sending, start at 100 Mbps and adjust.") }
            Section("Custom") {
                HStack {
                    TextField("Mbps", text: $limit).keyboardType(.numberPad)
                    Text("Mbps").foregroundStyle(.secondary)
                    Button("Set") { save(min(100000, max(0, Int(limit) ?? 0))) }.beamButton().controlSize(.small).disabled(Int(limit) == nil)
                }
            }
        }
        .scrollContentBackground(.hidden).background { BeamBackground() }
        .navigationTitle("Upload Limit").navigationBarTitleDisplayMode(.inline)
        .onAppear { if !presets.contains(current) { limit = String(current) } }
    }
    private func save(_ value: Int) {
        bridge.perform { try await bridge.updateSettings(patch: ["uploadLimitMbps": value]); Haptics.success() }
    }
}
struct DiagnosticsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var busy = false
    @State private var testing = false
    @State private var testResult: String?
    var body: some View {
        Form {
            Section {
                SettingToggle(title: "Share Diagnostics", symbol: "chart.bar.doc.horizontal.fill", color: .blue, key: "shareDiagnostics", value: bridge.settings?.shareDiagnostics)
                ActionRow(title: testing ? "Sending…" : "Send Test Report", symbol: "paperplane.fill", color: .green) {
                    testing = true; testResult = nil
                    bridge.perform { defer { testing = false }; let r: String = try await bridge.call("diagnosticsTest"); testResult = r; Haptics.success() }
                }.disabled(testing || bridge.settings?.shareDiagnostics == false)
                if let testResult { Text(testResult).font(.footnote).foregroundStyle(.secondary).textSelection(.enabled) }
            } footer: { Text("Sends a redacted error and performance summary about once a day, plus crash reports, so bugs get fixed. Never includes file names, file contents, messages or contacts. Send Test Report checks that reports get through.") }
            Section {
                SettingToggle(title: "Detailed Logging", symbol: "doc.text.magnifyingglass", color: .gray, key: "verboseLogging", value: bridge.settings?.verboseLogging)
                ActionRow(title: busy ? "Exporting…" : "Export Logs", symbol: "square.and.arrow.up.fill", color: .beam) {
                    busy = true; bridge.perform { defer { busy = false }; try await bridge.action("exportLogs") }
                }.disabled(busy)
            } footer: { Text("Detailed logging records extra network detail for reproducing a problem. Close and reopen DropBeam to apply. Export Logs lets you choose where to share them.") }
            Section { LinkRow(title: "Privacy Policy", symbol: "doc.text.fill", color: .blue, url: DropBeamLinks.privacy) }
        }
        .scrollContentBackground(.hidden).background { BeamBackground() }
        .navigationTitle("Diagnostics").navigationBarTitleDisplayMode(.inline)
    }
}
struct ConnectionInfoView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var busy = false
    @State private var result: String?
    var body: some View {
        List {
            Section {
                Button { busy = true; bridge.perform { defer { busy = false }; result = try await bridge.call("connectionTest") } } label: {
                    HStack { Text(busy ? "Testing…" : "Test Connection"); Spacer(); if busy { ProgressView() } }
                }.disabled(busy)
                if let result { Text(result).font(.subheadline).foregroundStyle(.secondary).textSelection(.enabled) }
            } footer: { Text("Every transfer is end-to-end encrypted. Keep both devices open until transfers finish.") }
            Section("Routes") {
                route("Local", "wifi", .green, "Same Wi-Fi or network. Files go straight across your network, never over the internet.")
                route("Direct", "arrow.left.arrow.right", .blue, "An encrypted peer-to-peer link across the internet.")
                route("Relay", "cloud.fill", .orange, "When no direct path exists, an encrypted relay carries the data. It can’t read it, but may be slower.")
                route("Connecting", "ellipsis", .gray, "Finding the best route to the other device.")
            }
            Section {
                Button("Open DropBeam Settings") { if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) } }
            } header: { Text("Local Network Access") } footer: {
                Text("If nearby transfers use the relay, make sure Local Network is on for DropBeam on both devices (iOS Settings → DropBeam, or Privacy & Security → Local Network).")
            }
        }
        .beamList()
        .navigationTitle("How Transfers Connect").navigationBarTitleDisplayMode(.inline)
    }
    private func route(_ title: String, _ symbol: String, _ color: Color, _ detail: String) -> some View {
        HStack(alignment: .top, spacing: 14) {
            RowIcon(symbol: symbol, color: color)
            VStack(alignment: .leading, spacing: 3) { Text(title).font(.body.weight(.semibold)).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }; Text(detail).font(.subheadline).foregroundStyle(.secondary) }
        }.padding(.vertical, 2).accessibilityElement(children: .combine)
    }
}
struct RecoverySettingsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var clearing = false
    @State private var keepDays: Int?
    @State private var budget: Double?
    var body: some View {
        Form {
            Section {
                Picker("Keep Copies For", selection: Binding(get: { keepDays ?? bridge.settings?.folderHistoryKeepDays ?? 30 }, set: { n in
                    keepDays = n
                    bridge.perform { await OptimisticSetting.save(key: "folderHistoryKeepDays", value: n, bridge: bridge) { keepDays = nil } }
                })) { Text("7 Days").tag(7); Text("30 Days").tag(30); Text("90 Days").tag(90); Text("Forever").tag(0) }
                Picker("Storage per Folder", selection: Binding(get: { budget ?? bridge.settings?.folderHistoryBudgetBytes ?? 2147483648 }, set: { n in
                    budget = n
                    bridge.perform { await OptimisticSetting.save(key: "folderHistoryBudgetBytes", value: n, bridge: bridge) { budget = nil } }
                })) { Text("500 MB").tag(524288000.0); Text("2 GB").tag(2147483648.0); Text("5 GB").tag(5368709120.0); Text("No Limit").tag(0.0) }
            } footer: { Text("When a file in a shared folder is deleted or replaced, DropBeam keeps a copy until these limits remove the oldest. Your live files are never touched. Browse copies in History → Recoverable.") }
            Section { Button("Free Up Space Now", role: .destructive) { clearing = true } }
        }
        .scrollContentBackground(.hidden).background { BeamBackground() }
        .navigationTitle("Recoverable Files").navigationBarTitleDisplayMode(.inline)
        .confirmationDialog("Permanently delete all saved copies?", isPresented: $clearing, titleVisibility: .visible) { Button("Delete Saved Copies", role: .destructive) { bridge.perform { let freed: Double = try await bridge.call("recoverableEmptyAll"); bridge.showToast("Freed \(Formatters.bytes(freed))") } } }
    }
}
/// Plain-language privacy summary + where your data lives and how to erase it
/// (App Review 5.1.1: DropBeam has no server account — identity is a key on the device).
struct PrivacyView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var confirmErase = false
    @State private var erasing = false
    var body: some View {
        List {
            Section {
                point("person.crop.circle.badge.xmark", .blue, "No account with us", "There’s no sign-up. Your identity is a key created on this iPhone.")
                point("lock.fill", .green, "End-to-end encrypted", "Files and messages go straight to the people you choose. Relays only pass along encrypted data.")
                point("iphone", .gray, "Stored on your devices", "Friends, chats and history live on this iPhone and your linked devices — never on our servers.")
                point("chart.bar.doc.horizontal.fill", .orange, "Diagnostics you control", "Redacted error summaries help fix bugs. Turn them off anytime in Diagnostics.")
            }
            Section {
                LinkRow(title: "Privacy Policy", symbol: "doc.text.fill", color: .blue, url: DropBeamLinks.privacy)
                NavigationLink { DiagnosticsView() } label: { RowLabel(title: "Diagnostics", symbol: "waveform.path.ecg", color: .red) }
            }
            Section {
                NavigationLink { DevicesView() } label: { RowLabel(title: "Linked Devices", symbol: "laptopcomputer.and.iphone", color: .gray) }
                NavigationLink { RecoverySettingsView() } label: { RowLabel(title: "Recoverable Files", symbol: "clock.arrow.circlepath", color: .teal) }
            } header: { Text("Your Data") } footer: {
                Text("Remove friends by swiping in Friends, clear transfers in History, and unlink this iPhone in Linked Devices.")
            }
            Section {
                Button(role: .destructive) { confirmErase = true } label: {
                    Label("Erase All Data", systemImage: "trash.fill").foregroundStyle(.red)
                }.disabled(erasing)
            } footer: {
                Text("Deletes everything DropBeam keeps on this iPhone — your DropBeam identity, friends, chats, history, settings and the files in the DropBeam folder — removes this iPhone from your linked devices and stops notifications. Your other devices keep their data. DropBeam then closes and starts fresh.")
            }
        }
        .beamList()
        .navigationTitle("Privacy & Your Data").navigationBarTitleDisplayMode(.inline)
        .confirmationDialog("Erase all DropBeam data on this iPhone?", isPresented: $confirmErase, titleVisibility: .visible) {
            Button("Erase All Data", role: .destructive) {
                erasing = true
                Task { await DataEraser.eraseAndQuit() }
            }
        } message: {
            Text("This can’t be undone. Received files in On My iPhone › DropBeam are deleted too; files in another folder you chose and photos already saved to Photos stay.")
        }
        .overlay {
            if erasing {
                ZStack {
                    Color.black.opacity(0.25).ignoresSafeArea()
                    VStack(spacing: 14) {
                        ProgressView()
                        Text("Erasing…").font(.headline)
                        Text("DropBeam will close when it’s done.").font(.footnote).foregroundStyle(.secondary)
                    }.padding(28).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
                }.accessibilityElement(children: .combine).accessibilityAddTraits(.updatesFrequently)
            }
        }
        #if targetEnvironment(simulator)
        // QA: `-showErase` opens the confirmation (no touch input in CI).
        .task { if CommandLine.arguments.contains("-showErase") { try? await Task.sleep(for: .seconds(1)); confirmErase = true } }
        #endif
    }
    private func point(_ symbol: String, _ color: Color, _ title: String, _ detail: String) -> some View {
        HStack(alignment: .top, spacing: 14) {
            RowIcon(symbol: symbol, color: color)
            VStack(alignment: .leading, spacing: 3) { Text(title).font(.body.weight(.semibold)).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }; Text(detail).font(.subheadline).foregroundStyle(.secondary) }
        }.padding(.vertical, 2).accessibilityElement(children: .combine)
    }
}

/// Settings → Transfers: routing and speed switches most people never touch.
struct TransferSettingsView: View {
    @EnvironmentObject private var bridge: Bridge
    var body: some View {
        List {
            Section {
                SettingToggle(title: "Direct Connections Only", symbol: "lock.shield.fill", color: .teal, key: "requireDirect", value: bridge.settings?.requireDirect)
                if bridge.settings?.waitForDirect != nil {
                    SettingToggle(title: "Wait for a Direct Link", symbol: "hourglass", color: .orange, key: "waitForDirect", value: bridge.settings?.requireDirect == true ? false : bridge.settings?.waitForDirect)
                        .disabled(bridge.settings?.requireDirect == true)
                }
                NavigationLink { ConnectionInfoView() } label: { RowLabel(title: "How Transfers Connect", symbol: "antenna.radiowaves.left.and.right", color: .blue) }
            } header: { Text("Connections") } footer: { Text("Direct Connections Only fails a send when no direct path can be made. Shared folders always use the best available path.") }
            Section {
                SettingToggle(title: "Parallel Streams", symbol: "square.stack.3d.up.fill", color: .indigo, key: "parallelStreams", value: bridge.settings?.parallelStreams)
                NavigationLink { UploadLimitView() } label: {
                    RowLabel(title: "Upload Limit", symbol: "speedometer", color: .orange, value: (bridge.settings?.uploadLimitMbps ?? 0) > 0 ? "\(Int(bridge.settings?.uploadLimitMbps ?? 0)) Mbps" : "None")
                }
                SettingToggle(title: "Show Speeds in Megabits", symbol: "gauge.with.dots.needle.67percent", color: .gray, key: "showMegabits", value: bridge.settings?.showMegabits)
            } header: { Text("Speed") } footer: { Text("Parallel streams can speed up files over 16 MB. Turn off if transfers stall.") }
            Section("Relay") {
                NavigationLink { TextSettingView(title: "Custom Relay", key: "customRelay", value: bridge.settings?.customRelay ?? "", placeholder: "https://relay.example.com", footer: "Use the same relay URL on both devices. Leave blank to use the public relays. Close and reopen DropBeam to apply.", link: ("Relay setup guide", DropBeamLinks.relaySetup)) } label: {
                    RowLabel(title: "Custom Relay", symbol: "server.rack", color: .gray, value: (bridge.settings?.customRelay ?? "").isEmpty ? "Off" : "On")
                }
            }
        }
        .beamList()
        .navigationTitle("Transfers").navigationBarTitleDisplayMode(.inline)
    }
}

/// Settings → Save Files To: the current folder, Choose Folder…, Reset.
struct SaveFolderView: View {
    @ObservedObject private var folder = SaveFolder.shared
    var body: some View {
        List {
            Section {
                HStack(spacing: 14) {
                    Image(systemName: "folder.fill").font(.system(size: 30)).foregroundStyle(.blue)
                        .frame(width: 44, height: 44).accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 2) {
                        Text(folder.displayName).font(.body.weight(.semibold)).lineLimit(2)
                        Text(folder.place).font(.subheadline).foregroundStyle(.secondary).lineLimit(2)
                    }
                }
                .padding(.vertical, 4).accessibilityElement(children: .combine)
                .accessibilityLabel("Current folder: \(folder.displayName), \(folder.place)")
                Button("Show in Files", systemImage: "arrow.up.forward.app") { folder.showInFiles() }
            } header: { Text("Current Folder") }
            Section {
                Button("Choose Folder…", systemImage: "folder.badge.gearshape") { Task { await folder.choose() } }
                if folder.custom != nil {
                    Button("Reset to DropBeam Folder", systemImage: "arrow.uturn.backward") { Haptics.tap(); folder.reset() }
                }
            } footer: {
                Text("Files you receive are saved here. Pick any folder in Files, including iCloud Drive. Files you already received stay where they are.")
            }
        }
        .beamList()
        .navigationTitle("Save Files To").navigationBarTitleDisplayMode(.inline)
    }
}
