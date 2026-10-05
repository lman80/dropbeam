import SwiftUI
import UIKit
import PhotosUI
import Vision
import VisionKit
import AVFoundation
import CoreImage.CIFilterBuiltins

// MARK: - Friend invites
//
// A friend code is `dropbeam:<base64url JSON {v, eid, name}>` (friends.rs). It has
// to carry the device key, so it can't be short without a server to look it up.
// What makes it painless instead:
// • Sharing sends a picture (QR + your name/photo, WhatsApp style) AND a short
//   message that contains the code — the friend pastes the whole message, or saves
//   the picture and scans it from Photos.
// • Add Friend accepts anything reasonable: the whole message, the code with or
//   without "dropbeam:", wrapped lines, a link, a QR photo (src/lib/codes.ts +
//   friends.rs user_code_body parse the same things).
// • The app registers the `dropbeam:` URL scheme, so a friend's QR scanned with
//   the Camera app (or a tapped link) opens DropBeam straight into Add Friend.

enum InviteText {
    /// A web page that turns `…/add/#<code>` into a `dropbeam:` link. Published from the
    /// separate public repo lman80/dropbeam-invite (GitHub Pages); source mirrored in docs/add/.
    static let webPage: URL? = URL(string: "https://lman80.github.io/dropbeam-invite/add/")
    static func message(name: String, code: String) -> String {
        var lines = ["Add me on DropBeam\(name.isEmpty ? "" : " — I’m \(name)")."]
        if let page = webPage { lines.append("Tap to add me: \(page.absoluteString)#\(code)") }
        lines.append("Or copy this whole message, open DropBeam, tap Add Friend and then Paste. You can also save the picture and choose Scan from Photo.")
        lines.append("DropBeam connects our phones directly (no server in between), so keep it open until we’re connected.")
        lines.append(code)
        return lines.joined(separator: "\n\n")
    }
}

enum QRCodeImage {
    static func make(_ text: String, scale: CGFloat = 12) -> UIImage? {
        let filter = CIFilter.qrCodeGenerator()
        filter.message = Data(text.utf8); filter.correctionLevel = "M"
        guard let output = filter.outputImage else { return nil }
        let scaled = output.transformed(by: CGAffineTransform(scaleX: scale, y: scale))
        guard let cg = CIContext().createCGImage(scaled, from: scaled.extent) else { return nil }
        return UIImage(cgImage: cg)
    }
}

/// Reads a QR code out of a photo or screenshot (Vision, on device).
enum QRPhotoReader {
    enum Failure: LocalizedError {
        case unreadable, none
        var errorDescription: String? {
            switch self {
            case .unreadable: return "That photo couldn’t be opened. Try another one."
            case .none: return "No QR code found in that photo. Choose a screenshot of a DropBeam invite."
            }
        }
    }
    /// Every QR payload in the image, DropBeam codes first.
    static func codes(in data: Data) async throws -> [String] {
        try await Task.detached(priority: .userInitiated) {
            guard let source = CGImageSourceCreateWithData(data as CFData, nil),
                  let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                      kCGImageSourceCreateThumbnailFromImageAlways: true,
                      kCGImageSourceCreateThumbnailWithTransform: true,
                      kCGImageSourceThumbnailMaxPixelSize: 2400
                  ] as CFDictionary) else { throw Failure.unreadable }
            let request = VNDetectBarcodesRequest()
            request.symbologies = [.qr]
            var found: [String] = []
            if (try? VNImageRequestHandler(cgImage: image).perform([request])) != nil {
                found = (request.results ?? []).compactMap(\.payloadStringValue).filter { !$0.isEmpty }
            }
            // Vision needs the Neural Engine/GPU; Core Image's detector is the fallback
            // (and catches a QR Vision misses in a busy screenshot).
            if found.isEmpty, let detector = CIDetector(ofType: CIDetectorTypeQRCode, context: nil, options: [CIDetectorAccuracy: CIDetectorAccuracyHigh]) {
                found = detector.features(in: CIImage(cgImage: image)).compactMap { ($0 as? CIQRCodeFeature)?.messageString }.filter { !$0.isEmpty }
            }
            guard !found.isEmpty else { throw Failure.none }
            return found.sorted { a, _ in a.lowercased().contains("dropbeam") || a.lowercased().hasPrefix("direct") }
        }.value
    }
}

/// Presents the system share sheet from whatever is on top (sheets included).
@MainActor enum SystemShare {
    static func present(_ items: [Any]) {
        guard let scene = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).first(where: { $0.activationState == .foregroundActive }) ?? UIApplication.shared.connectedScenes.first as? UIWindowScene,
              var top = scene.windows.first(where: { $0.windowLevel == .normal && !$0.isHidden })?.rootViewController else { return }
        while let next = top.presentedViewController, !next.isBeingDismissed { top = next }
        let controller = UIActivityViewController(activityItems: items, applicationActivities: nil)
        controller.popoverPresentationController?.sourceView = top.view
        controller.popoverPresentationController?.sourceRect = CGRect(x: top.view.bounds.midX, y: top.view.bounds.maxY - 80, width: 1, height: 1)
        top.present(controller, animated: true)
    }
}

/// The invite as a picture: photo/monogram, name, QR, one line of how-to.
/// Rendered to an image for sharing, and shown on the My Code screen.
struct InviteCard: View {
    let name: String
    let code: String
    var avatar: UIImage?
    var colorSeed: String
    var body: some View {
        VStack(spacing: 0) {
            Group {
                if let avatar { Image(uiImage: avatar).resizable().scaledToFill() }
                else {
                    ZStack {
                        AvatarPalette.color(for: colorSeed)
                        Text(AvatarPalette.initials(name.isEmpty ? "DropBeam" : name)).font(.system(size: 30, weight: .semibold, design: .rounded)).foregroundStyle(.white)
                    }
                }
            }
            .frame(width: 76, height: 76).clipShape(Circle())
            .overlay(Circle().strokeBorder(.white, lineWidth: 3))
            .padding(.bottom, 10)
            Text(name.isEmpty ? "DropBeam" : name).font(.system(size: 24, weight: .bold, design: .rounded)).foregroundStyle(.black)
                .lineLimit(2).multilineTextAlignment(.center).minimumScaleFactor(0.7)
            Text("Add me on DropBeam").font(.system(size: 16, weight: .medium)).foregroundStyle(Color(white: 0.42)).padding(.top, 2)
            Group {
                if let qr = QRCodeImage.make(code) { Image(uiImage: qr).interpolation(.none).resizable().scaledToFit() }
                else { Image(systemName: "qrcode").resizable().scaledToFit().foregroundStyle(.black) }
            }
            .frame(width: 210, height: 210).padding(14)
            .background(.white, in: RoundedRectangle(cornerRadius: 20, style: .continuous))
            .overlay(RoundedRectangle(cornerRadius: 20, style: .continuous).strokeBorder(Color(white: 0.9), lineWidth: 1))
            .padding(.top, 18)
            Text("In DropBeam, tap **Add Friend → Scan from Photo** and choose this picture — or scan it with your camera.")
                .font(.system(size: 13)).foregroundStyle(Color(white: 0.38)).multilineTextAlignment(.center)
                .fixedSize(horizontal: false, vertical: true).padding(.top, 16).padding(.horizontal, 6)
            HStack(spacing: 6) {
                AppIconImage(size: 18)
                Text("DropBeam").font(.system(size: 13, weight: .semibold)).foregroundStyle(Color(white: 0.3))
            }.padding(.top, 14)
        }
        .padding(.horizontal, 24).padding(.vertical, 26)
        .frame(width: 320)
        .background(Color(white: 0.965), in: RoundedRectangle(cornerRadius: 30, style: .continuous))
        .environment(\.colorScheme, .light)
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Your DropBeam invite: \(name). A QR code friends can scan to add you.")
    }
}

@MainActor enum InviteShare {
    /// The card as a PNG-backed image (3x) — lands in Messages as a picture.
    static func image(name: String, code: String, avatar: UIImage?, colorSeed: String) -> UIImage? {
        let renderer = ImageRenderer(content: InviteCard(name: name, code: code, avatar: avatar, colorSeed: colorSeed).padding(20).background(Color.white))
        renderer.scale = 3
        return renderer.uiImage
    }
    static func share(bridge: Bridge, code: String) async {
        let name = bridge.settings?.displayName ?? ""
        var avatar: UIImage?
        if let raw = bridge.settings?.avatar {
            let path = LocalPaths.resolve(raw)
            avatar = await ThumbnailProvider.shared.image(path: path, points: 120, tag: FriendAvatar.version(path))?.image
        }
        var items: [Any] = []
        if let image = image(name: name, code: code, avatar: avatar, colorSeed: name.isEmpty ? "you" : name) { items.append(image) }
        items.append(InviteText.message(name: name, code: code))
        SystemShare.present(items)
    }
}

// MARK: - My Code

/// "Your DropBeam code": the invite card + Share Invite / Copy. Shared by Profile and
/// Friends → My Code. While it's open, anyone who adds you shows up right here.
struct MyCodeSection: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var code = ""
    @State private var codeError: String?
    @State private var avatar: UIImage?
    @State private var baseline: Set<String>?
    @State private var joined: [Friend] = []
    private var name: String { bridge.settings?.displayName ?? "" }
    var body: some View {
        Section {
            VStack(spacing: 18) {
                if let codeError {
                    ContentUnavailableView { Label("Couldn’t Load Your Code", systemImage: "qrcode") } description: { Text(codeError) } actions: { Button("Try Again", action: load).beamButton() }
                } else if code.isEmpty { ProgressView().frame(height: 300) }
                else {
                    InviteCard(name: name, code: code, avatar: avatar, colorSeed: name.isEmpty ? "you" : name)
                        .shadow(color: .black.opacity(0.06), radius: 12, y: 4)
                }
                HStack(spacing: 12) {
                    Button { Haptics.tap(); Task { await InviteShare.share(bridge: bridge, code: code) } } label: {
                        Label("Share", systemImage: "square.and.arrow.up").frame(maxWidth: .infinity)
                    }.beamButton(prominent: true).disabled(code.isEmpty)
                    Button { UIPasteboard.general.string = InviteText.message(name: name, code: code); Haptics.success(); bridge.showToast("Invite copied — paste it in a message") } label: {
                        Label("Copy", systemImage: "doc.on.doc").frame(maxWidth: .infinity)
                    }.beamButton().disabled(code.isEmpty)
                }.controlSize(.large)
                ForEach(joined) { friend in
                    Label("\(friend.displayName) added you", systemImage: "checkmark.circle.fill")
                        .font(.subheadline.weight(.semibold)).foregroundStyle(.green)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }.frame(maxWidth: .infinity).padding(.vertical, 12)
                .animation(.smooth, value: joined.map(\.id))
        } header: { Text("Your Invite") } footer: {
            Text("Share it in Messages or anywhere. DropBeam connects phones directly — there’s no server — so keep DropBeam open while your friend adds you.")
        }
        .task { load() }
        .task(id: bridge.settings?.avatar ?? "") {
            guard let raw = bridge.settings?.avatar else { avatar = nil; return }
            let path = LocalPaths.resolve(raw)
            avatar = await ThumbnailProvider.shared.image(path: path, points: 120, tag: FriendAvatar.version(path))?.image
        }
        .onAppear { if baseline == nil { baseline = Set(bridge.friends.map(\.id)) } }
        .onChange(of: bridge.friends.map(\.id)) { _, ids in
            guard let baseline else { return }
            let fresh = bridge.friends.filter { !baseline.contains($0.id) && !$0.ownDevice && $0.groupedUnder == nil }
            if fresh.map(\.id) != joined.map(\.id) { joined = fresh; if !fresh.isEmpty { Haptics.success() } }
            _ = ids
        }
    }
    private func load() {
        codeError = nil
        Task { do { code = try await bridge.myInviteCode() } catch { codeError = error.localizedDescription } }
    }
}

// MARK: - Add Friend

/// Add a friend from anything: camera, a screenshot/photo of their invite, the
/// pasted message or code, or a `dropbeam:` link that opened the app. After adding,
/// it shows the live connection ("Waiting for Alex…" → "Connected").
struct AddFriendSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    /// A code handed in (a tapped link / Camera-scanned QR): added right away.
    var initialCode: String?
    private enum Phase: Equatable { case entry, working, added(id: String, name: String, code: String?) }
    @State private var phase: Phase = .entry
    @State private var error: String?
    @State private var typing = false
    @State private var typed = ""
    @State private var photo: PhotosPickerItem?
    @State private var cameraAllowed = false
    @State private var cameraDenied = false
    @State private var scannerFailed = false
    @State private var sharing = false
    @FocusState private var fieldFocused: Bool
    private var scannerReady: Bool { cameraAllowed && !scannerFailed && DataScannerViewController.isSupported && DataScannerViewController.isAvailable }
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 18) {
                    switch phase {
                    case .entry, .working: entry
                    case let .added(id, name, code): FriendConnectStatus(friendID: id, name: name, code: code)
                    }
                }.padding(20)
            }
            .scrollDismissesKeyboard(.interactively)
            .navigationTitle(phaseIsAdded ? "" : "Add Friend").navigationBarTitleDisplayMode(.inline).beamCanvas()
            .toolbar {
                if phaseIsAdded { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
                else { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() }.disabled(phase == .working) } }
            }
            .task {
                if let initialCode { submit(initialCode); return }
                guard DataScannerViewController.isSupported else { return }
                switch AVCaptureDevice.authorizationStatus(for: .video) {
                case .authorized: cameraAllowed = true
                case .notDetermined: cameraAllowed = await AVCaptureDevice.requestAccess(for: .video)
                default: cameraAllowed = false
                }
                cameraDenied = !cameraAllowed
            }
            .onChange(of: photo) { _, item in if let item { readPhoto(item) } }
        }.tint(.beam).interactiveDismissDisabled(phase == .working)
    }
    private var phaseIsAdded: Bool { if case .added = phase { return true }; return false }
    @ViewBuilder private var entry: some View {
        if scannerReady {
            ZStack {
                QRScanner { value in if phase == .entry { Haptics.success(); submit(value) } } failure: { _ in scannerFailed = true }
                RoundedRectangle(cornerRadius: 24, style: .continuous).stroke(.white.opacity(0.9), lineWidth: 3).padding(48).allowsHitTesting(false)
                if phase == .working { ProgressView().controlSize(.large).tint(.white).padding(20).background(.ultraThinMaterial, in: Circle()) }
            }
            .frame(height: 300).clipShape(RoundedRectangle(cornerRadius: 28, style: .continuous))
            .accessibilityElement().accessibilityLabel("Camera viewfinder. Point it at your friend’s DropBeam QR code.")
            Text("Point the camera at your friend’s QR code.").font(.subheadline).foregroundStyle(.secondary)
        } else {
            VStack(spacing: 10) {
                Image(systemName: "person.crop.circle.badge.plus").font(.system(size: 52, weight: .light)).foregroundStyle(.tint)
                Text("Add a Friend").font(.title2.bold())
                Text("Paste the invite they sent you, or choose a screenshot of their QR code.")
                    .font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
                if cameraDenied {
                    Button("Allow Camera to Scan Codes") { if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) } }
                        .font(.subheadline.weight(.semibold)).padding(.top, 2)
                }
            }.padding(.vertical, 12)
        }
        VStack(spacing: 10) {
            // PasteButton: the tap itself is consent — no "Allow Paste" prompt.
            PasteButton(payloadType: String.self) { strings in
                Task { @MainActor in if let text = strings.first { submit(text) } }
            }
            .buttonBorderShape(.capsule).controlSize(.large).tint(.beam)
            .frame(maxWidth: .infinity)
            .disabled(phase == .working)
            PhotosPicker(selection: $photo, matching: .images, photoLibrary: .shared()) {
                Label("Scan from Photo", systemImage: "photo.on.rectangle").frame(maxWidth: .infinity, minHeight: 36)
            }.beamButton().disabled(phase == .working)
            if typing {
                HStack(spacing: 8) {
                    TextField("Invite or code", text: $typed, axis: .vertical).lineLimit(1...4)
                        .textInputAutocapitalization(.never).autocorrectionDisabled()
                        .focused($fieldFocused).submitLabel(.go).onSubmit { submit(typed) }
                    Button("Add") { submit(typed) }.beamButton(prominent: true).controlSize(.small)
                        .disabled(typed.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || phase == .working)
                }
                .padding(.leading, 14).padding(.trailing, 6).padding(.vertical, 6).frame(minHeight: 50)
                .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            } else {
                Button("Type a Code Instead") { typing = true; fieldFocused = true; Haptics.tap() }.font(.subheadline.weight(.semibold)).padding(.top, 2)
            }
        }
        if phase == .working && !scannerReady { ProgressView("Adding…").padding(.top, 4) }
        if let error {
            Label(error, systemImage: "exclamationmark.triangle.fill").font(.subheadline).foregroundStyle(.red)
                .frame(maxWidth: .infinity, alignment: .leading).accessibilityAddTraits(.updatesFrequently)
            // A device-link code isn't a friend (S1): only Settings → Devices links devices.
            if Bridge.isDeviceCodeMessage(error) {
                Button { dismiss(); bridge.openDevicesSettings() } label: {
                    Label("Open Settings → Devices", systemImage: "laptopcomputer.and.iphone").frame(maxWidth: .infinity, minHeight: 36)
                }.beamButton()
            }
        }
        PeerToPeerNote(text: "DropBeam connects your phones directly — there’s no server. Your friend needs DropBeam open too, until you’re connected.")
            .padding(.top, 6)
        Button { sharing = true } label: { Label("Send Them Your Invite Instead", systemImage: "square.and.arrow.up") }
            .font(.subheadline.weight(.semibold))
            .sheet(isPresented: $sharing) { MyCodeSheet().environmentObject(bridge) }
    }
    private func readPhoto(_ item: PhotosPickerItem) {
        error = nil; phase = .working
        Task {
            defer { photo = nil }
            do {
                guard let data = try await item.loadTransferable(type: Data.self) else { throw QRPhotoReader.Failure.unreadable }
                let found = try await QRPhotoReader.codes(in: data)
                submit(found[0])
            } catch { self.error = error.localizedDescription; phase = .entry; Haptics.warning() }
        }
    }
    private func submit(_ raw: String) {
        let text = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !text.isEmpty else { return }
        // Never link a device from here (S1); the engine refuses too.
        if Bridge.isLinkCode(text) { error = Bridge.deviceCodeElsewhere; phase = .entry; fieldFocused = false; Haptics.warning(); return }
        error = nil; phase = .working; fieldFocused = false
        Task {
            do {
                let result: OpenCodeResult = try await bridge.call("openAnyCode", ["code": text])
                switch result.kind {
                case "friend":
                    Haptics.success()
                    withAnimation(.smooth) { phase = .added(id: result.friendId ?? "", name: result.name ?? "Your friend", code: result.code) }
                case "folderInvite":
                    let accepted: Bool = try await bridge.call("acceptFolderInvite", ["code": result.code ?? text])
                    if accepted { bridge.showToast("Joined shared folder") }
                    dismiss()
                default:
                    bridge.showToast("Receiving files…"); dismiss()
                }
            } catch {
                self.error = error.localizedDescription; phase = .entry; Haptics.warning()
            }
        }
    }
}

/// After adding a friend: their avatar and a live status. Checks every few seconds
/// until they're online, then says hello again so the friendship is two-way even if
/// they were offline when added.
struct FriendConnectStatus: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let friendID: String
    let name: String
    /// Their code: re-adding it once they're online re-sends our hello (a no-op for the list).
    var code: String?
    @State private var online = false
    @State private var reintroduced = false
    private var friend: Friend { bridge.friends.first { $0.id == friendID } ?? Friend(id: friendID, name: name) }
    var body: some View {
        VStack(spacing: 14) {
            ContactAvatar(friend: friend, size: 96).padding(.top, 12)
            Text("You added \(friend.displayName)").font(.title2.bold()).multilineTextAlignment(.center)
            HStack(spacing: 8) {
                if friend.awaitingAccept == true {
                    Image(systemName: "hourglass").foregroundStyle(.secondary)
                    Text("Waiting for \(friend.displayName) to accept you")
                } else if online || bridge.presence[friendID] == true {
                    Image(systemName: "checkmark.circle.fill").foregroundStyle(.green)
                    Text("Connected").fontWeight(.semibold)
                } else {
                    ProgressView().controlSize(.small)
                    Text("Waiting for \(friend.displayName) to come online…")
                }
            }
            .font(.subheadline).padding(.horizontal, 16).padding(.vertical, 10)
            .background(Color(uiColor: .secondarySystemGroupedBackground), in: Capsule())
            .accessibilityElement(children: .combine).accessibilityAddTraits(.updatesFrequently)
            .animation(.smooth, value: online)
            if friend.awaitingAccept == true {
                PeerToPeerNote(text: "\(friend.displayName) gets a friend request. Once they tap Accept, your messages reach them. You can close this.")
            } else if !(online || bridge.presence[friendID] == true) {
                PeerToPeerNote(text: "Ask \(friend.displayName) to open DropBeam. You can close this — you’ll connect automatically the next time you’re both online.")
            }
            Button { bridge.perform { try await bridge.openChat(friendId: friendID) }; dismiss() } label: {
                Label("Message \(friend.displayName)", systemImage: "message").frame(maxWidth: .infinity, minHeight: 36)
            }.beamButton(prominent: true).controlSize(.large).padding(.top, 8)
        }
        .frame(maxWidth: .infinity)
        .task(id: friendID) {
            guard !friendID.isEmpty else { return }
            // ~3 minutes of gentle checks; presence snapshots also flip it live.
            for _ in 0..<45 {
                if Task.isCancelled { return }
                if let check = try? await bridge.pingFriend(id: friendID), check.online == true {
                    online = true; Haptics.success()
                    if !reintroduced, let code, code.lowercased().hasPrefix("dropbeam:") { reintroduced = true; try? await bridge.addFriendByCode(code: code) }
                    return
                }
                try? await Task.sleep(for: .seconds(4))
            }
        }
    }
}

/// One calm line explaining the peer-to-peer rule, with a symbol — used wherever
/// two people need to be online at the same time.
struct PeerToPeerNote: View {
    let text: String
    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "point.3.connected.trianglepath.dotted").font(.body).foregroundStyle(.secondary).frame(width: 22)
            Text(text).font(.footnote).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
        }.frame(maxWidth: .infinity, alignment: .leading)
            .accessibilityElement(children: .combine)
    }
}
