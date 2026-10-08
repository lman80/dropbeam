import SwiftUI
import UIKit
import PhotosUI
import Vision
import UserNotifications

/// First-run setup (new installs only — see Bridge.onboarding): what DropBeam is and
/// how peer-to-peer works, your name, an optional photo, your other devices, a first
/// friend, then notifications — one question per screen, every step skippable.
struct OnboardingFlow: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    enum Step: Int, CaseIterable { case welcome, name, photo, devices, recovery, friend, notifications }
    @State private var step: Step = .welcome
    @State private var forward = true
    @State private var joining = false
    @State private var restoring = false
    @State private var notificationsAsked = false
    var body: some View {
        VStack(spacing: 0) {
            topBar
            ZStack {
                page(step).id(step)
                    .transition(reduceMotion ? .opacity : .asymmetric(
                        insertion: .move(edge: forward ? .trailing : .leading).combined(with: .opacity),
                        removal: .move(edge: forward ? .leading : .trailing).combined(with: .opacity)))
            }.frame(maxWidth: .infinity, maxHeight: .infinity)
        }
        .background { BeamBackground() }
        .tint(.beam)
        .sheet(isPresented: $joining, onDismiss: {
            // Linked into an account: name and photo came with it.
            if !bridge.needsName, (bridge.myDevice?.linked.count ?? 0) > 1 { go(.friend) }
        }) { JoinAccountSheet().environmentObject(bridge) }
        .sheet(isPresented: $restoring) {
            // Restored: the account is back; the person still picks their name.
            RestoreRecoverySheet { go(.name) }.environmentObject(bridge)
        }
        .task {
            let settings = await UNUserNotificationCenter.current().notificationSettings()
            notificationsAsked = settings.authorizationStatus != .notDetermined
            #if targetEnvironment(simulator)
            let args = ProcessInfo.processInfo.arguments
            if let i = args.firstIndex(of: "-onboardingStep"), i + 1 < args.count, let n = Int(args[i + 1]), let s = Step(rawValue: n) { step = s }
            #endif
        }
    }
    @ViewBuilder private var topBar: some View {
        HStack {
            if step.rawValue > Step.name.rawValue {
                Button { back() } label: { Image(systemName: "chevron.left").font(.body.weight(.semibold)).frame(width: 44, height: 44) }
                    .accessibilityLabel("Back")
            } else { Color.clear.frame(width: 44, height: 44) }
            Spacer()
            if step != .welcome {
                HStack(spacing: 6) {
                    ForEach(Step.allCases.dropFirst(), id: \.self) { s in
                        Capsule().fill(s.rawValue <= step.rawValue ? Color.beam : Color(uiColor: .tertiarySystemFill))
                            .frame(width: s == step ? 22 : 8, height: 8)
                    }
                }
                .animation(.smooth, value: step)
                .accessibilityElement().accessibilityLabel("Step \(step.rawValue) of \(Step.allCases.count - 1)")
            }
            Spacer()
            if step.rawValue > Step.name.rawValue {
                Button("Skip") { finish() }.font(.body).frame(minWidth: 44, minHeight: 44).accessibilityLabel("Skip setup")
            } else { Color.clear.frame(width: 44, height: 44) }
        }.padding(.horizontal, 8).frame(height: 52)
    }
    @ViewBuilder private func page(_ step: Step) -> some View {
        switch step {
        case .welcome: WelcomeStep(next: { go(.name) }, join: { joining = true }, restore: { restoring = true })
        case .name: NameStep(next: { go(.photo) })
        case .photo: PhotoStep(next: { go(.devices) })
        case .devices: DevicesStep(next: { go(.recovery) })
        case .recovery: RecoveryStep(next: { go(.friend) })
        case .friend: FriendStep(next: { notificationsAsked ? finish() : go(.notifications) })
        case .notifications: NotificationsStep(next: finish)
        }
    }
    private func go(_ next: Step) {
        Haptics.tap(); forward = next.rawValue > step.rawValue
        withAnimation(reduceMotion ? .easeInOut(duration: 0.2) : .smooth(duration: 0.4)) { step = next }
    }
    private func back() {
        guard let previous = Step(rawValue: step.rawValue - 1), previous != .welcome else { return }
        go(previous)
    }
    private func finish() {
        Haptics.success()
        // Skipping from any step still leaves a name friends can see.
        if bridge.needsName {
            let name = bridge.settings?.displayName?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
            Task { try? await bridge.action("setDisplayName", ["name": name.isEmpty ? UIDevice.current.name : name]) }
        }
        bridge.onboarding = false
        bridge.needsName = false
    }
}

// MARK: - Page chrome

/// One onboarding page: scrolling content, buttons pinned at the bottom.
private struct OnboardingPage<Content: View, Actions: View>: View {
    @ViewBuilder var content: Content
    @ViewBuilder var actions: Actions
    var body: some View {
        ScrollView {
            VStack(spacing: 22) { content }
                .padding(.horizontal, 28).padding(.top, 12).padding(.bottom, 24)
                .frame(maxWidth: 560).frame(maxWidth: .infinity)
        }
        .scrollBounceBehavior(.basedOnSize)
        .safeAreaInset(edge: .bottom) {
            VStack(spacing: 10) { actions }
                .padding(.horizontal, 28).padding(.top, 12).padding(.bottom, 8)
                .frame(maxWidth: 560).frame(maxWidth: .infinity)
                .background {
                    Color(uiColor: .systemGroupedBackground).ignoresSafeArea()
                        .overlay(alignment: .top) {
                            LinearGradient(colors: [Color(uiColor: .systemGroupedBackground).opacity(0), Color(uiColor: .systemGroupedBackground)], startPoint: .top, endPoint: .bottom)
                                .frame(height: 24).offset(y: -24).allowsHitTesting(false)
                        }
                }
        }
    }
}
private struct PageTitle: View {
    let title: String
    let detail: String
    var body: some View {
        VStack(spacing: 10) {
            Text(title).font(.largeTitle.bold()).multilineTextAlignment(.center).fixedSize(horizontal: false, vertical: true)
            Text(detail).font(.body).foregroundStyle(.secondary).multilineTextAlignment(.center).fixedSize(horizontal: false, vertical: true)
        }.accessibilityElement(children: .combine).accessibilityAddTraits(.isHeader)
    }
}
private struct PrimaryButton: View {
    let title: String
    var busy = false
    let action: () -> Void
    var body: some View {
        Button(action: action) {
            HStack(spacing: 8) { if busy { ProgressView() }; Text(title).font(.headline) }.frame(maxWidth: .infinity, minHeight: 30)
        }.beamButton(prominent: true).controlSize(.large).disabled(busy)
    }
}
private struct SecondaryButton: View {
    let title: String
    let action: () -> Void
    var body: some View {
        Button(title, action: action).font(.body.weight(.semibold)).frame(maxWidth: .infinity, minHeight: 44)
    }
}
private struct FeatureRow: View {
    let symbol: String
    let title: String
    let detail: String
    var body: some View {
        HStack(alignment: .top, spacing: 16) {
            Image(systemName: symbol).font(.title2).foregroundStyle(.tint).frame(width: 32)
            VStack(alignment: .leading, spacing: 3) {
                Text(title).font(.headline)
                Text(detail).font(.subheadline).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Spacer(minLength: 0)
        }.accessibilityElement(children: .combine)
    }
}

// MARK: - 1. Welcome

private struct WelcomeStep: View {
    let next: () -> Void
    let join: () -> Void
    let restore: () -> Void
    var body: some View {
        OnboardingPage {
            PeerToPeerDemo().padding(.top, 8)
            PageTitle(title: "Welcome to DropBeam", detail: "Send photos, videos and files to friends and your own devices.")
            VStack(alignment: .leading, spacing: 14) {
                // Three short rows so the last one — the permission to say yes to — is
                // on screen without scrolling.
                FeatureRow(symbol: "lock.fill", title: "Private, device to device", detail: "Straight to the other device, encrypted.")
                FeatureRow(symbol: "antenna.radiowaves.left.and.right", title: "Both need DropBeam open", detail: "They open DropBeam too, then it connects.")
                FeatureRow(symbol: "wifi", title: "Tap Allow when asked", detail: "When your iPhone asks to find nearby devices, tap Allow.")
            }.padding(.top, 4)
        } actions: {
            PrimaryButton(title: "Get Started", action: next)
            SecondaryButton(title: "I Already Use DropBeam", action: join)
            Button("Lost your old phone? Restore with Recovery Code", action: restore)
                .font(.footnote).frame(maxWidth: .infinity, minHeight: 44)
        }
    }
}

/// A short, looping, non-interactive picture of peer-to-peer: a photo travels from
/// this phone straight to a computer while the cloud stays crossed out. With Reduce
/// Motion it's a still picture.
struct PeerToPeerDemo: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    private let period = 2.8
    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 60, paused: reduceMotion)) { timeline in
            let t = reduceMotion ? 0.5 : (timeline.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: period)) / period
            // 0–0.12 appear, 0.12–0.72 travel, 0.72–0.85 land, then rest.
            let travel = min(1, max(0, (t - 0.12) / 0.6))
            let eased = travel < 0.5 ? 2 * travel * travel : 1 - pow(-2 * travel + 2, 2) / 2
            let visible = reduceMotion ? 1 : (t < 0.12 ? t / 0.12 : t < 0.78 ? 1 : max(0, 1 - (t - 0.78) / 0.07))
            let landed = !reduceMotion && t > 0.72 && t < 0.95
            GeometryReader { geo in
                let w = geo.size.width, disc: CGFloat = 86
                let start = disc / 2 + 10, end = w - disc / 2 - 10
                ZStack {
                    Capsule().fill(Color(uiColor: .separator)).frame(width: max(0, end - start - disc), height: 2)
                        .position(x: w / 2, y: 108)
                    VStack(spacing: 4) {
                        Image(systemName: "icloud.slash").font(.system(size: 22, weight: .regular)).foregroundStyle(.secondary)
                        Text("No server").font(.caption.weight(.medium)).foregroundStyle(.secondary)
                    }.position(x: w / 2, y: 36)
                    device("iphone", label: "You").position(x: start, y: 108)
                    device("laptopcomputer", label: "Friend").scaleEffect(landed ? 1.06 : 1).position(x: end, y: 108)
                    RoundedRectangle(cornerRadius: 8, style: .continuous).fill(Color.beam)
                        .frame(width: 34, height: 40)
                        .overlay(Image(systemName: "photo.fill").font(.system(size: 15)).foregroundStyle(.white))
                        .shadow(color: .black.opacity(0.12), radius: 4, y: 2)
                        .opacity(visible)
                        .position(x: start + (end - start) * eased, y: 108 - sin(eased * .pi) * 26)
                }
            }
        }
        .frame(height: 172)
        .animation(.spring(response: 0.3, dampingFraction: 0.6), value: reduceMotion)
        .accessibilityElement().accessibilityLabel("A photo travels straight from your phone to a friend’s computer, with no server in between.")
    }
    private func device(_ symbol: String, label: String) -> some View {
        VStack(spacing: 8) {
            Image(systemName: symbol).font(.system(size: 34, weight: .light)).foregroundStyle(.primary)
                .frame(width: 86, height: 86)
                .background(Color(uiColor: .secondarySystemGroupedBackground), in: Circle())
                .overlay(alignment: .topTrailing) { Circle().fill(.green).frame(width: 14, height: 14).overlay(Circle().stroke(Color(uiColor: .systemGroupedBackground), lineWidth: 3)).offset(x: -6, y: 6) }
            Text(label).font(.caption.weight(.medium)).foregroundStyle(.secondary)
        }.offset(y: 12)
    }
}

// MARK: - 2. Name

private struct NameStep: View {
    @EnvironmentObject private var bridge: Bridge
    let next: () -> Void
    @State private var name = ""
    @State private var busy = false
    @State private var error: String?
    @FocusState private var focused: Bool
    private var trimmed: String { name.trimmingCharacters(in: .whitespacesAndNewlines) }
    var body: some View {
        OnboardingPage {
            Image(systemName: "person.crop.circle").font(.system(size: 64, weight: .light)).foregroundStyle(.tint).padding(.top, 20).accessibilityHidden(true)
            PageTitle(title: "What’s your name?", detail: "Friends see it when you send files and chat.")
            TextField("Your name", text: $name).textContentType(.name).textInputAutocapitalization(.words)
                .font(.title3).multilineTextAlignment(.center)
                .submitLabel(.continue).onSubmit(save).focused($focused)
                .padding(.horizontal, 16).frame(minHeight: 56)
                .background(Color(uiColor: .secondarySystemGroupedBackground), in: RoundedRectangle(cornerRadius: 14, style: .continuous))
            if let error { Label(error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red).font(.subheadline) }
        } actions: {
            PrimaryButton(title: "Continue", busy: busy, action: save).disabled(trimmed.isEmpty)
        }
        .onAppear {
            // A real name the engine already has (e.g. an account) — never "iPhone".
            let current = bridge.settings?.displayName ?? ""
            if name.isEmpty, !current.isEmpty, !current.lowercased().hasPrefix("iphone"), !current.lowercased().hasPrefix("ipad"), !current.lowercased().contains("my iphone") { name = current }
            focused = true
        }
    }
    private func save() {
        guard !trimmed.isEmpty, !busy else { return }
        busy = true; focused = false
        Task {
            defer { busy = false }
            do { try await bridge.action("setDisplayName", ["name": trimmed]); next() }
            catch { self.error = error.localizedDescription; Haptics.warning() }
        }
    }
}

// MARK: - 3. Photo

private struct PhotoStep: View {
    @EnvironmentObject private var bridge: Bridge
    let next: () -> Void
    @State private var item: PhotosPickerItem?
    @State private var busy = false
    @State private var error: String?
    private var hasPhoto: Bool { bridge.settings?.avatar != nil }
    var body: some View {
        OnboardingPage {
            PhotosPicker(selection: $item, matching: .images, photoLibrary: .shared()) {
                MyAvatar(size: 148)
                    .overlay { if busy { Circle().fill(.black.opacity(0.35)); ProgressView().tint(.white) } }
                    .overlay(alignment: .bottomTrailing) {
                        Image(systemName: "camera.fill").font(.body.weight(.semibold)).foregroundStyle(.white)
                            .frame(width: 42, height: 42).background(Color.beam, in: Circle())
                            .overlay(Circle().stroke(Color(uiColor: .systemGroupedBackground), lineWidth: 4))
                    }
            }.buttonStyle(.plain).padding(.top, 20).accessibilityLabel(hasPhoto ? "Change profile photo" : "Choose a profile photo")
            PageTitle(title: "Add a profile photo", detail: "Friends see it next to your name, on every device.")
            if let error { Label(error, systemImage: "exclamationmark.triangle.fill").foregroundStyle(.red).font(.subheadline) }
        } actions: {
            if hasPhoto {
                PrimaryButton(title: "Continue", action: next)
                PhotosPicker(selection: $item, matching: .images, photoLibrary: .shared()) { Text("Choose a Different Photo").font(.body.weight(.semibold)).frame(maxWidth: .infinity, minHeight: 44) }
            } else {
                PhotosPicker(selection: $item, matching: .images, photoLibrary: .shared()) {
                    HStack(spacing: 8) { if busy { ProgressView() }; Text("Choose Photo").font(.headline) }.frame(maxWidth: .infinity, minHeight: 30)
                }.beamButton(prominent: true).controlSize(.large).disabled(busy)
                SecondaryButton(title: "Not Now", action: next)
            }
        }
        .onChange(of: item) { _, picked in if let picked { use(picked) } }
    }
    private func use(_ picked: PhotosPickerItem) {
        busy = true; error = nil
        Task {
            defer { busy = false; item = nil }
            do {
                guard let data = try await picked.loadTransferable(type: Data.self) else { throw AvatarCrop.Failure() }
                let path = try await AvatarCrop.squareJPEG(from: data)
                // Same engine command as desktop (set_profile_avatar): stored, synced to
                // your devices and sent to friends with your name.
                try await bridge.action("setAvatar", ["path": path])
                Haptics.success()
            } catch { self.error = error.localizedDescription; Haptics.warning() }
        }
    }
}

/// Crops a photo to a square around the main face (else the centre), 1024 px JPEG.
enum AvatarCrop {
    struct Failure: LocalizedError { var errorDescription: String? { "That photo couldn’t be used. Try another one." } }
    static func squareJPEG(from data: Data) async throws -> String {
        try await Task.detached(priority: .userInitiated) {
            guard let source = CGImageSourceCreateWithData(data as CFData, nil),
                  let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                      kCGImageSourceCreateThumbnailFromImageAlways: true,
                      kCGImageSourceCreateThumbnailWithTransform: true,
                      kCGImageSourceThumbnailMaxPixelSize: 2048
                  ] as CFDictionary) else { throw Failure() }
            let w = CGFloat(image.width), h = CGFloat(image.height)
            let side = min(w, h)
            var center = CGPoint(x: w / 2, y: h / 2)
            let faces = VNDetectFaceRectanglesRequest()
            if (try? VNImageRequestHandler(cgImage: image).perform([faces])) != nil,
               let face = faces.results?.max(by: { $0.boundingBox.width * $0.boundingBox.height < $1.boundingBox.width * $1.boundingBox.height }) {
                // Vision's origin is bottom-left; nudge up a little so the head isn't cut.
                center = CGPoint(x: face.boundingBox.midX * w, y: (1 - face.boundingBox.midY) * h - face.boundingBox.height * h * 0.1)
            }
            let x = min(max(0, center.x - side / 2), w - side), y = min(max(0, center.y - side / 2), h - side)
            guard let square = image.cropping(to: CGRect(x: x, y: y, width: side, height: side).integral) else { throw Failure() }
            let out = CGSize(width: min(1024, side), height: min(1024, side))
            let format = UIGraphicsImageRendererFormat(); format.scale = 1
            let jpeg = UIGraphicsImageRenderer(size: out, format: format).jpegData(withCompressionQuality: 0.88) { _ in
                UIImage(cgImage: square).draw(in: CGRect(origin: .zero, size: out))
            }
            let dir = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("dropbeam-picked", isDirectory: true)
            try FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
            let url = dir.appendingPathComponent("profile-\(UUID().uuidString).jpg")
            try jpeg.write(to: url, options: .atomic)
            return url.path
        }.value
    }
}

// MARK: - 4. Other devices

private struct DevicesStep: View {
    @EnvironmentObject private var bridge: Bridge
    let next: () -> Void
    @State private var linking = false
    private var linked: Bool { (bridge.myDevice?.linked.count ?? 0) > 1 }
    var body: some View {
        OnboardingPage {
            HStack(spacing: 18) {
                ForEach(["laptopcomputer", "iphone", "desktopcomputer"], id: \.self) { symbol in
                    Image(systemName: symbol).font(.system(size: 30, weight: .light))
                        .frame(width: 74, height: 74)
                        .background(Color(uiColor: .secondarySystemGroupedBackground), in: Circle())
                }
            }.padding(.top, 24).accessibilityHidden(true)
            PageTitle(title: linked ? "Your devices are linked" : "Do you have other devices?",
                      detail: linked ? "Friends, chats, your name and photo now stay in sync between them."
                                     : "Link your Mac, PC or another phone to share your friends and chats between them. Send files to them in one tap.")
            if !linked {
                PeerToPeerNote(text: "On your computer, open DropBeam → Settings → Devices → Link a Device. A square code appears. Then tap Link a Device below and choose Scan the Other Device.")
            }
        } actions: {
            if linked { PrimaryButton(title: "Continue", action: next) }
            else {
                PrimaryButton(title: "Link a Device") { linking = true }
                SecondaryButton(title: "Not Now", action: next)
            }
        }
        .sheet(isPresented: $linking) { LinkDeviceSheet(start: .show, title: "Link a Device").environmentObject(bridge) }
    }
}

// MARK: - 4b. Recovery code

/// Offered once, right after setup: write down the 12 words. Skippable; always
/// in Settings → Devices later. Skipped on its own when the code is already
/// saved (a restore, or a device that joined an account whose code was saved here).
private struct RecoveryStep: View {
    @EnvironmentObject private var bridge: Bridge
    let next: () -> Void
    @State private var saving = false
    @State private var saved = false
    var body: some View {
        OnboardingPage {
            Image(systemName: "key.horizontal").font(.system(size: 60, weight: .light)).foregroundStyle(.tint).padding(.top, 24).accessibilityHidden(true)
            PageTitle(title: saved ? "Your code is saved" : "Save a recovery code",
                      detail: saved ? "Keep the paper somewhere safe, like with your important papers."
                                    : "A few words on paper. If you ever lose all your phones and computers, they bring back your friends and chats.")
        } actions: {
            if saved { PrimaryButton(title: "Continue", action: next) }
            else {
                PrimaryButton(title: "Save My Code") { saving = true }
                SecondaryButton(title: "Later") { Task { try? await bridge.recoveryLater() }; next() }
            }
        }
        .task { if (try? await bridge.recoveryStatus())?.saved == true { saved = true } }
        .sheet(isPresented: $saving, onDismiss: { Task { if (try? await bridge.recoveryStatus())?.saved == true { saved = true } } }) {
            SaveRecoverySheet().environmentObject(bridge)
        }
    }
}

// MARK: - 5. First friend

private struct FriendStep: View {
    @EnvironmentObject private var bridge: Bridge
    let next: () -> Void
    @State private var adding = false
    @State private var sharing = false
    @State private var baseline = Set<String>()
    private var added: [Friend] { bridge.friends.filter { !baseline.contains($0.id) && !$0.ownDevice && $0.groupedUnder == nil } }
    var body: some View {
        OnboardingPage {
            if added.isEmpty {
                Image(systemName: "person.2.fill").font(.system(size: 54, weight: .light)).foregroundStyle(.tint).padding(.top, 24).accessibilityHidden(true)
            } else {
                HStack(spacing: -10) { ForEach(added.prefix(4)) { ContactAvatar(friend: $0, size: 72).overlay(Circle().stroke(Color(uiColor: .systemGroupedBackground), lineWidth: 3)) } }.padding(.top, 24)
            }
            PageTitle(title: added.isEmpty ? "Add your first friend" : "You’re connected with \(ListFormatter.localizedString(byJoining: added.map(\.displayName)))",
                      detail: added.isEmpty ? "Send them your invite in Messages, or add the invite they sent you." : "Send them something, or add more friends any time from the Friends tab.")
            VStack(spacing: 10) {
                Button { share() } label: { Label("Share My Invite", systemImage: "square.and.arrow.up").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                Button { adding = true } label: { Label("Add a Friend’s Invite", systemImage: "person.badge.plus").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
            }.controlSize(.large)
            PeerToPeerNote(text: "DropBeam is peer-to-peer, so there’s no server to hold messages. Keep it open on both phones until you’re connected.")
        } actions: {
            if added.isEmpty { SecondaryButton(title: "Not Now", action: next) }
            else { PrimaryButton(title: "Continue", action: next) }
        }
        .onAppear { if baseline.isEmpty { baseline = Set(bridge.friends.map(\.id)) } }
        .sheet(isPresented: $adding) { AddFriendSheet().environmentObject(bridge) }
    }
    private func share() {
        Haptics.tap()
        bridge.perform { await InviteShare.share(bridge: bridge, code: try await bridge.myInviteCode()) }
    }
}

// MARK: - 6. Notifications (+ the Local Network explanation)

private struct NotificationsStep: View {
    let next: () -> Void
    @State private var busy = false
    var body: some View {
        OnboardingPage {
            Image(systemName: "bell.badge.fill").font(.system(size: 58)).foregroundStyle(.tint).symbolRenderingMode(.hierarchical).padding(.top, 24).accessibilityHidden(true)
            PageTitle(title: "Know when files arrive", detail: "Get a notification when someone sends you files or a message.")
            HStack(alignment: .top, spacing: 12) {
                Image(systemName: "wifi").font(.body).foregroundStyle(.secondary).frame(width: 22)
                Text("If iOS asks to find devices on your local network, tap **Allow** — nearby devices then connect directly at full speed.")
                    .font(.footnote).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }.frame(maxWidth: .infinity, alignment: .leading).padding(.top, 6)
        } actions: {
            PrimaryButton(title: "Turn On Notifications", busy: busy) {
                busy = true
                Task {
                    _ = try? await UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound, .badge])
                    PushRegistration.permissionGranted()
                    busy = false; next()
                }
            }
            SecondaryButton(title: "Not Now", action: next)
        }
    }
}
