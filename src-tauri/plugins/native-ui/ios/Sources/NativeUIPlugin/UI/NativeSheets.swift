import SwiftUI
import VisionKit
import Vision
import AVFoundation

struct QRScannerSheet: View {
    @Environment(\.dismiss) private var dismiss
    let title: String
    /// Connect as soon as a QR code is recognized (no extra Continue tap).
    var autoSubmit = false
    var hint = "Point the camera at a DropBeam QR code, or paste the code."
    let submit: (String) async throws -> Void
    @State private var paste = false
    @State private var code = ""
    @State private var busy = false
    @State private var error: String?
    @State private var cameraAllowed = false
    @State private var cameraDenied = false
    @State private var scannerError: String?
    @FocusState private var fieldFocused: Bool
    private var scannerReady: Bool { cameraAllowed && DataScannerViewController.isSupported && DataScannerViewController.isAvailable && scannerError == nil }
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 20) {
                    if scannerReady && !paste {
                        ZStack {
                            QRScanner { value in if !busy { code = value; Haptics.success(); if autoSubmit { connect() } else { paste = true } } } failure: { scannerError = $0; paste = true }
                            RoundedRectangle(cornerRadius: 28, style: .continuous).stroke(.white.opacity(0.9), lineWidth: 3).padding(44).allowsHitTesting(false)
                            if busy { ProgressView().controlSize(.large).tint(.white).padding(20).background(.ultraThinMaterial, in: Circle()) }
                        }
                        .frame(height: 360).clipShape(RoundedRectangle(cornerRadius: 28, style: .continuous))
                        .accessibilityElement().accessibilityLabel("Camera viewfinder. Point it at a DropBeam QR code.")
                        Text(hint).font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
                    } else if !paste {
                        unavailable
                    }
                    if paste || !scannerReady {
                        VStack(alignment: .leading, spacing: 12) {
                            Text("DropBeam Code").font(.footnote.weight(.semibold)).foregroundStyle(.secondary).textCase(.uppercase)
                            TextField("dropbeam:…", text: $code, axis: .vertical)
                                .textInputAutocapitalization(.never).autocorrectionDisabled().font(.body.monospaced())
                                .lineLimit(2...5).focused($fieldFocused).submitLabel(.go)
                                .padding(14).background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                            HStack(spacing: 12) {
                                PasteButton(payloadType: String.self) { strings in
                                    Task { @MainActor in code = strings.first?.trimmingCharacters(in: .whitespacesAndNewlines) ?? "" }
                                }.buttonBorderShape(.capsule).tint(.beam).frame(maxWidth: .infinity)
                                Button(action: connect) { Text(busy ? "Connecting…" : "Continue").frame(maxWidth: .infinity, minHeight: 32) }
                                    .beamButton(prominent: true).disabled(busy || code.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                            }
                        }
                    }
                    if let error {
                        Label(error, systemImage: "exclamationmark.triangle.fill").font(.subheadline).foregroundStyle(.red)
                            .frame(maxWidth: .infinity, alignment: .leading).accessibilityAddTraits(.updatesFrequently)
                    }
                    if scannerReady {
                        Button(paste ? "Scan a QR Code Instead" : "Enter Code Instead") { paste.toggle(); error = nil; Haptics.tap(); if paste { fieldFocused = true } }
                            .font(.subheadline.weight(.semibold))
                    }
                }.padding(20)
            }
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle(title).navigationBarTitleDisplayMode(.inline).beamCanvas()
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) } }
            .task {
                guard DataScannerViewController.isSupported else { paste = true; return }
                switch AVCaptureDevice.authorizationStatus(for: .video) {
                case .authorized: cameraAllowed = true
                case .notDetermined: cameraAllowed = await AVCaptureDevice.requestAccess(for: .video)
                default: cameraAllowed = false
                }
                cameraDenied = !cameraAllowed
                if !cameraAllowed { paste = true }
            }
        }.tint(.beam).interactiveDismissDisabled(busy)
    }
    @ViewBuilder private var unavailable: some View {
        ContentUnavailableView {
            Label(cameraDenied ? "Camera Access Is Off" : "Scanning Unavailable", systemImage: "qrcode.viewfinder")
        } description: {
            Text(scannerError ?? (cameraDenied ? "Allow camera access in Settings to scan codes, or paste the code below." : "Paste a DropBeam code below."))
        } actions: {
            if cameraDenied { Button("Open Settings") { if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) } }.beamButton() }
        }
    }
    private func connect() {
        guard !busy else { return }; busy = true; error = nil
        Task {
            defer { busy = false }
            do { try await submit(code.trimmingCharacters(in: .whitespacesAndNewlines)); Haptics.success(); dismiss() }
            catch { self.error = error.localizedDescription; Haptics.warning(); paste = true }
        }
    }
}
private struct QRScanner: UIViewControllerRepresentable {
    let recognized: (String) -> Void
    let failure: (String) -> Void
    func makeCoordinator() -> Coordinator { Coordinator(recognized, failure) }
    func makeUIViewController(context: Context) -> DataScannerViewController {
        let controller = DataScannerViewController(recognizedDataTypes: [.barcode(symbologies: [.qr])], qualityLevel: .balanced, recognizesMultipleItems: false, isHighFrameRateTrackingEnabled: false, isPinchToZoomEnabled: true, isGuidanceEnabled: true, isHighlightingEnabled: true)
        controller.delegate = context.coordinator
        return controller
    }
    func updateUIViewController(_ controller: DataScannerViewController, context: Context) {
        guard !controller.isScanning else { return }
        do { try controller.startScanning() } catch { DispatchQueue.main.async { failure(error.localizedDescription) } }
    }
    static func dismantleUIViewController(_ controller: DataScannerViewController, coordinator: Coordinator) { controller.stopScanning() }
    final class Coordinator: NSObject, DataScannerViewControllerDelegate {
        let recognized: (String) -> Void
        let failure: (String) -> Void
        private var delivered = false
        init(_ recognized: @escaping (String) -> Void, _ failure: @escaping (String) -> Void) { self.recognized = recognized; self.failure = failure }
        func dataScanner(_ dataScanner: DataScannerViewController, didAdd addedItems: [RecognizedItem], allItems: [RecognizedItem]) { receive(addedItems) }
        func dataScanner(_ dataScanner: DataScannerViewController, didTapOn item: RecognizedItem) { receive([item]) }
        func dataScanner(_ dataScanner: DataScannerViewController, becameUnavailableWithError error: DataScannerViewController.ScanningUnavailable) { failure("Camera scanning stopped. Paste a code to continue.") }
        private func receive(_ items: [RecognizedItem]) {
            guard !delivered else { return }
            for item in items { if case .barcode(let barcode) = item, let code = barcode.payloadStringValue { delivered = true; recognized(code); return } }
        }
    }
}

/// "Send to…" chooser after picking files from the Send tab or the share sheet.
struct SendToSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let paths: [String]
    @State private var busy = false
    @State private var error: String?
    private func mine(_ friend: Friend) -> Bool { if friend.ownDevice { return true }; guard let account = bridge.myDevice?.accountPub, !account.isEmpty else { return false }; return friend.accountPub == account }
    private var people: [Friend] { bridge.friends.filter { $0.groupedUnder == nil } }
    var body: some View {
        NavigationStack {
            List {
                Section {
                    HStack(spacing: 14) {
                        if paths.count == 1, let path = paths.first, LocalMedia(path: path) != nil {
                            MediaThumbnail(path: path, width: 44, height: 44).clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                        } else { FileGlyph(name: paths.first ?? "", symbol: paths.count > 1 ? "doc.on.doc" : nil) }
                        VStack(alignment: .leading, spacing: 2) {
                            Text(paths.count == 1 ? (paths[0] as NSString).lastPathComponent : "\(paths.count) items").font(.headline).lineLimit(2)
                            Text(sizeLabel).font(.subheadline).foregroundStyle(.secondary)
                        }
                    }.accessibilityElement(children: .combine)
                }
                if let error { Section { Label(error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red) } }
                group("My Devices", friends: people.filter(mine), empty: "Link your other devices in Settings → My Devices.")
                group("Friends", friends: people.filter { !mine($0) }, empty: "Add a friend to send by name.")
                Section {
                    Button { send(nil) } label: {
                        HStack(spacing: 14) {
                            RowIcon(symbol: "qrcode", color: .beam)
                            VStack(alignment: .leading, spacing: 2) { Text("Quick Send with a Code").foregroundStyle(.primary); Text("Anyone with DropBeam can receive").font(.subheadline).foregroundStyle(.secondary) }
                        }.contentShape(Rectangle())
                    }.buttonStyle(.plain)
                } footer: { Text("You’ll get a code and QR to share. The files stay on this iPhone until someone receives them.") }
            }
            .beamList()
            .disabled(busy)
            .overlay { if busy { ProgressView("Sending…").padding(24).background(.regularMaterial, in: RoundedRectangle(cornerRadius: 20)) } }
            .navigationTitle("Send To").navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) } }
            .task { try? await bridge.action("refreshRecipients") }
        }.tint(.beam).interactiveDismissDisabled(busy)
    }
    private var sizeLabel: String {
        let total = paths.reduce(0.0) { sum, path in sum + ((try? FileManager.default.attributesOfItem(atPath: path)[.size] as? NSNumber)?.doubleValue ?? 0) }
        return total > 0 ? Formatters.bytes(total) : "Ready to send"
    }
    @ViewBuilder private func group(_ title: String, friends: [Friend], empty: String) -> some View {
        Section(title) {
            if friends.isEmpty { Text(empty).foregroundStyle(.secondary) }
            ForEach(friends) { friend in
                Button { send(friend.id) } label: {
                    HStack(spacing: 14) {
                        ContactAvatar(friend: friend, size: 42)
                        VStack(alignment: .leading, spacing: 2) { Text(friend.displayName).font(.body.weight(.semibold)).foregroundStyle(.primary).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }; PresenceLabel(online: bridge.presence[friend.id] == true) }
                        Spacer()
                        Image(systemName: "paperplane.fill").foregroundStyle(.tint)
                    }.contentShape(Rectangle())
                }.buttonStyle(.plain).accessibilityLabel("Send to \(friend.displayName), \(bridge.presence[friend.id] == true ? "online" : "offline")")
            }
        }
    }
    private func send(_ friend: String?) {
        busy = true; error = nil
        Task {
            defer { busy = false }
            do {
                if let friend { try await bridge.sendToFriend(friendId: friend, paths: paths) }
                else { try await bridge.action("quickSend", ["paths": paths]) }
                bridge.pendingSend = []; Haptics.success(); dismiss()
            } catch { self.error = error.localizedDescription; Haptics.warning() }
        }
    }
}

struct OnboardingSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var name = ""
    @State private var busy = false
    @State private var error: String?
    @State private var joining = false
    @FocusState private var focused: Bool
    private var valid: Bool { !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }
    var body: some View {
        ScrollView {
            VStack(spacing: 28) {
                VStack(spacing: 16) {
                    Image(systemName: "paperplane.fill").font(.system(size: 46, weight: .semibold)).foregroundStyle(.white)
                        .frame(width: 96, height: 96)
                        .background(LinearGradient(colors: [.beam, .blue], startPoint: .topLeading, endPoint: .bottomTrailing), in: RoundedRectangle(cornerRadius: 24, style: .continuous))
                        .shadow(color: .beam.opacity(0.35), radius: 18, y: 8).accessibilityHidden(true)
                    Text("Welcome to DropBeam").font(.largeTitle.bold()).multilineTextAlignment(.center)
                    Text("Send photos and files straight to friends and your own devices — fast, private, end-to-end encrypted.")
                        .font(.body).foregroundStyle(.secondary).multilineTextAlignment(.center)
                }.padding(.top, 36)
                VStack(alignment: .leading, spacing: 10) {
                    Text("What should friends call you?").font(.headline)
                    TextField("Your name", text: $name).textContentType(.name).textInputAutocapitalization(.words)
                        .submitLabel(.continue).onSubmit(save).focused($focused)
                        .padding(14).background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
                    Text("Shown when you send files and chat. You can change it anytime in Settings.").font(.footnote).foregroundStyle(.secondary)
                }
                if let error { Label(error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red).font(.subheadline) }
                Button(action: save) { Text(busy ? "Saving…" : "Continue").font(.headline).frame(maxWidth: .infinity, minHeight: 36) }
                    .beamButton(prominent: true).controlSize(.large).disabled(busy || !valid)
                VStack(spacing: 8) {
                    Text("Already use DropBeam on another device?").font(.subheadline).foregroundStyle(.secondary)
                    Button { joining = true; Haptics.tap() } label: { Label("Link to Your Account", systemImage: "qrcode.viewfinder") }
                        .font(.subheadline.weight(.semibold))
                }.padding(.top, 4)
            }.padding(24).frame(maxWidth: 520).frame(maxWidth: .infinity)
        }
        .scrollDismissesKeyboard(.interactively)
        .beamCanvas().tint(.beam).interactiveDismissDisabled()
        .onAppear { name = bridge.settings?.displayName ?? "" }
        .sheet(isPresented: $joining) { JoinAccountSheet() }
    }
    private func save() {
        guard valid, !busy else { return }
        busy = true; focused = false
        Task { defer { busy = false }; do { try await bridge.action("setDisplayName", ["name": name]); Haptics.success(); bridge.needsName = false } catch { self.error = error.localizedDescription } }
    }
}

struct FolderInviteSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let invite: FolderInvite
    @State private var busy = false
    @State private var error: String?
    var body: some View {
        NavigationStack {
            ContentUnavailableView {
                Label(invite.folderName, systemImage: "folder.fill.badge.person.crop")
            } description: {
                Text("\(invite.fromName) wants to share this folder with you. Choose a folder in Files to keep in sync with theirs.")
            } actions: {
                VStack(spacing: 12) {
                    Button {
                        busy = true
                        Task { defer { busy = false }; do { let accepted: Bool = try await bridge.call("acceptFolderInvite", ["code": invite.code]); if accepted { Haptics.success(); bridge.showToast("Joined shared folder"); dismiss() } } catch { self.error = error.localizedDescription } }
                    } label: { Text(busy ? "Joining…" : "Choose Folder & Join").frame(minWidth: 220, minHeight: 32) }
                        .beamButton(prominent: true).disabled(busy)
                    if let error { Text(error).font(.subheadline).foregroundStyle(.red) }
                }
            }
            .beamCanvas()
            .navigationTitle("Shared Folder Invite").navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Decline") { dismiss() }.disabled(busy) } }
        }.tint(.beam).interactiveDismissDisabled(busy).presentationDetents([.medium, .large])
    }
}
