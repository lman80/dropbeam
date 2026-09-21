import SwiftUI
import VisionKit
import Vision
import AVFoundation

struct QRScannerSheet: View {
    @Environment(\.dismiss) private var dismiss
    let title: String
    let submit: (String) async throws -> Void
    @State private var paste = false
    @State private var code = ""
    @State private var busy = false
    @State private var error: String?
    @State private var cameraAllowed = false
    @State private var scannerError: String?
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 24) {
                    if cameraAllowed && DataScannerViewController.isSupported && DataScannerViewController.isAvailable && scannerError == nil && !paste {
                        ZStack {
                            QRScanner { value in if !busy { code = value; paste = true; Haptics.tap() } } failure: { scannerError = $0; paste = true }
                            RoundedRectangle(cornerRadius: 24).stroke(.white.opacity(0.85), lineWidth: 2).padding(36).allowsHitTesting(false)
                            VStack { Spacer(); Text("Place the QR code inside the frame").font(.subheadline).padding(12).background(.ultraThinMaterial, in: Capsule()).padding() }.allowsHitTesting(false)
                        }.frame(height: 340).clipShape(RoundedRectangle(cornerRadius: 24)).accessibilityLabel("QR code camera")
                    } else if !paste {
                        BeamEmpty(symbol: "qrcode.viewfinder", title: "A code connects you.", detail: scannerError ?? "Camera scanning is unavailable. Paste a DropBeam code below.")
                    }
                    GlassCard {
                        VStack(alignment: .leading, spacing: 16) {
                            Button(paste ? "Scan QR Code" : "Paste code instead") { paste.toggle(); Haptics.tap() }.beamButton()
                            if paste {
                                TextField("DropBeam code", text: $code, axis: .vertical).textInputAutocapitalization(.never).autocorrectionDisabled().padding(14).background(.quaternary, in: RoundedRectangle(cornerRadius: 14))
                                Button(busy ? "Connecting…" : "Continue", action: connect).beamButton(prominent: true).disabled(busy || code.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
                            }
                            if let error { Text(error).foregroundStyle(.red).font(.subheadline) }
                        }
                    }
                }.padding(20)
            }.navigationTitle(title).navigationBarTitleDisplayMode(.inline).beamCanvas()
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) } }
                .task {
                    guard DataScannerViewController.isSupported else { paste = true; return }
                    switch AVCaptureDevice.authorizationStatus(for: .video) {
                    case .authorized: cameraAllowed = true
                    case .notDetermined: cameraAllowed = await AVCaptureDevice.requestAccess(for: .video)
                    default: cameraAllowed = false
                    }
                    if !cameraAllowed { paste = true }
                }
        }.tint(.beam).interactiveDismissDisabled(busy)
    }
    private func connect() {
        guard !busy else { return }; busy = true; error = nil
        Task {
            defer { busy = false }
            do { try await submit(code.trimmingCharacters(in: .whitespacesAndNewlines)); Haptics.tap(); dismiss() }
            catch { self.error = error.localizedDescription }
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

struct SendToSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let paths: [String]
    @State private var busy = false
    @State private var error: String?
    private func mine(_ friend: Friend) -> Bool { guard let account = bridge.myDevice?.accountPub, !account.isEmpty else { return false }; return friend.accountPub == account }
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    Text(paths.count == 1 ? (paths[0] as NSString).lastPathComponent : "\(paths.count) files").font(.subheadline).foregroundStyle(.secondary)
                    group("My Devices", friends: bridge.friends.filter(mine))
                    group("Friends", friends: bridge.friends.filter { !mine($0) })
                    Button { send(nil) } label: { GlassCard { HStack { FileGlyph(name: "", symbol: "qrcode"); Text("Quick Send (code)").font(.headline); Spacer(); Image(systemName: "chevron.right") } } }.buttonStyle(.plain)
                    if let error { Text(error).foregroundStyle(.red) }
                }.padding(20).disabled(busy)
            }.navigationTitle("Send to").beamCanvas()
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(busy) } }
                .task { try? await bridge.action("refreshRecipients") }
        }.tint(.beam).interactiveDismissDisabled(busy)
    }
    private func group(_ title: String, friends: [Friend]) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title).font(.title2.weight(.semibold))
            if friends.isEmpty { GlassCard { Text(title == "My Devices" ? "Your linked devices appear here." : "Add a friend to send by name.").foregroundStyle(.secondary) } }
            ForEach(friends) { friend in
                Button { send(friend.id) } label: {
                    GlassCard { HStack(spacing: 14) { FriendAvatar(friend: friend); VStack(alignment: .leading, spacing: 5) { Text(friend.name).font(.headline).foregroundStyle(.primary); PresenceLabel(online: bridge.presence[friend.id] == true) }; Spacer(); Image(systemName: "chevron.right").foregroundStyle(.tertiary) } }
                }.buttonStyle(.plain)
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
                bridge.pendingSend = []; Haptics.tap(); dismiss()
            } catch { self.error = error.localizedDescription }
        }
    }
}

struct OnboardingSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var name = ""
    @State private var busy = false
    @State private var error: String?
    var body: some View {
        ScrollView {
            VStack(spacing: 28) {
                Image(systemName: "paperplane.fill").font(.system(size: 64, weight: .light)).foregroundStyle(.tint).padding(.top, 40)
                Text("Welcome to DropBeam").font(.largeTitle.bold()).multilineTextAlignment(.center)
                GlassCard {
                    VStack(alignment: .leading, spacing: 16) {
                        Text("What should people call you?").font(.headline)
                        TextField("Your name", text: $name).textContentType(.name).textInputAutocapitalization(.words).padding(14).background(.quaternary, in: RoundedRectangle(cornerRadius: 14))
                        Text("Friends see this name when you send files and chat. You can change it anytime in Settings.").foregroundStyle(.secondary)
                    }
                }
                if let error { Text(error).foregroundStyle(.red) }
                Button(busy ? "Saving…" : "Continue") {
                    busy = true
                    Task { defer { busy = false }; do { try await bridge.action("setDisplayName", ["name": name]); bridge.needsName = false } catch { self.error = error.localizedDescription } }
                }.frame(minHeight: 44).beamButton(prominent: true).disabled(busy || name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }.padding(24)
        }.beamCanvas().tint(.beam).interactiveDismissDisabled().onAppear { name = bridge.settings?.displayName ?? "" }
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
            ScrollView {
                VStack(spacing: 24) {
                    BeamEmpty(symbol: "folder.badge.person.crop", title: invite.folderName, detail: "\(invite.fromName) wants to share this folder with you. Import a folder into DropBeam in Files to keep its local copy in sync.")
                    Button(busy ? "Joining…" : "Accept & Import Folder") {
                        busy = true
                        Task { defer { busy = false }; do { let accepted: Bool = try await bridge.call("acceptFolderInvite", ["code": invite.code]); if accepted { bridge.showToast("Joined shared folder"); dismiss() } } catch { self.error = error.localizedDescription } }
                    }.beamButton(prominent: true).disabled(busy)
                    if let error { Text(error).foregroundStyle(.red) }
                }.padding(20)
            }.navigationTitle("Folder Invite").beamCanvas()
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Decline") { dismiss() }.disabled(busy) } }
        }.interactiveDismissDisabled(busy)
    }
}
