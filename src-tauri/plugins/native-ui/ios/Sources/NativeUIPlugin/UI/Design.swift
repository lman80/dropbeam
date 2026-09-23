import SwiftUI
import UIKit

@MainActor enum Haptics { static func tap() { UIImpactFeedbackGenerator(style: .light).impactOccurred() } }
extension Color {
    static let beam = Color(uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark
            ? UIColor(red: 124/255, green: 124/255, blue: 1, alpha: 1)
            : UIColor(red: 91/255, green: 91/255, blue: 240/255, alpha: 1)
    })
}
struct BeamBackground: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @State private var drift = false
    var body: some View {
        ZStack {
            Color(uiColor: .systemBackground)
            if #available(iOS 18, *) {
                MeshGradient(width: 3, height: 3,
                    points: [[0,0], [0.5,0], [1,0], [0,0.5], [drift ? 0.65 : 0.35,0.5], [1,0.5], [0,1], [0.5,1], [1,1]],
                    colors: [.beam, .blue, .clear, .clear, .beam, .blue, .blue, .clear, .beam])
                    .opacity(0.13)
            } else {
                LinearGradient(colors: [.beam.opacity(0.13), .clear, .blue.opacity(0.08)], startPoint: .topLeading, endPoint: .bottomTrailing)
            }
        }
        .ignoresSafeArea()
        .onAppear { if !reduceMotion { withAnimation(.easeInOut(duration: 9).repeatForever(autoreverses: true)) { drift = true } } }
    }
}
struct GlassCard<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        if #available(iOS 26, *) {
            content.padding(20).frame(maxWidth: .infinity, alignment: .leading)
                .glassEffect(.regular, in: .rect(cornerRadius: 24))
        } else {
            content.padding(20).frame(maxWidth: .infinity, alignment: .leading)
                .background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: 24))
        }
    }
}
struct GlassGroup<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        if #available(iOS 26, *) { GlassEffectContainer(spacing: 18) { content } }
        else { content }
    }
}
struct BeamButtonStyle: ViewModifier {
    var prominent = false
    @ViewBuilder func body(content: Content) -> some View {
        if #available(iOS 26, *) {
            if prominent { content.buttonStyle(.glassProminent) }
            else { content.buttonStyle(.glass) }
        } else {
            if prominent { content.buttonStyle(.borderedProminent) }
            else { content.buttonStyle(.bordered) }
        }
    }
}
extension View {
    func beamButton(prominent: Bool = false) -> some View { modifier(BeamButtonStyle(prominent: prominent)) }
    func beamCanvas() -> some View { background { BeamBackground() }.toolbarBackground(.hidden, for: .navigationBar) }
}
struct FriendAvatar: View {
    let friend: Friend
    var size: CGFloat = 52
    @State private var avatar: UIImage?
    var body: some View {
        Group {
            if let avatar {
                Image(uiImage: avatar).resizable().scaledToFill()
            } else {
                ZStack {
                    LinearGradient(colors: [.beam, .blue.opacity(0.8)], startPoint: .topLeading, endPoint: .bottomTrailing)
                    Text(friend.name.split(separator: " ").prefix(2).compactMap(\.first).map(String.init).joined().uppercased())
                        .font(.system(size: size * 0.33, weight: .semibold)).foregroundStyle(.white)
                }
            }
        }
        .frame(width: size, height: size).clipShape(Circle()).accessibilityHidden(true)
        .task(id: "\(friend.avatar ?? "")|\(size)") {
            avatar = nil
            guard let path = friend.avatar else { return }
            let result = await ThumbnailProvider.shared.image(path: path, points: size)
            if !Task.isCancelled { avatar = result?.image }
        }
    }
}
struct PresenceLabel: View {
    let online: Bool
    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(online ? Color.green : .secondary).frame(width: 6, height: 6)
            Text(online ? "Online now" : "Offline").font(.footnote).foregroundStyle(.secondary)
        }
    }
}
/// SF Symbol for a device, preferring what the OS says it is.
func deviceSymbol(_ kind: String?, os: String?) -> String {
    switch (os, kind) {
    case ("ios", "tablet"): return "ipad"
    case ("ios", _): return "iphone"
    case ("macos", "desktop"): return "desktopcomputer"
    case ("macos", _): return "laptopcomputer"
    case ("windows", _): return "pc"
    default: return deviceSymbol(kind) ?? "desktopcomputer"
    }
}
/// "iPhone", "Mac", "PC"… (mirrors deviceNoun in src/lib/deviceIcons.ts).
func deviceNoun(_ kind: String?, os: String?) -> String {
    switch os {
    case "ios": return kind == "tablet" ? "iPad" : "iPhone"
    case "macos": return "Mac"
    case "windows": return "PC"
    case "linux": return "Linux PC"
    default: return kind == "phone" ? "Phone" : kind == "tablet" ? "Tablet" : "Computer"
    }
}
/// An own device's avatar: its SF Symbol on a soft glass disc, like Blip's "Your devices".
struct DeviceAvatar: View {
    let kind: String?
    let os: String?
    var size: CGFloat = 52
    var body: some View {
        ZStack {
            Circle().fill(LinearGradient(colors: [Color(uiColor: .systemGray5), Color(uiColor: .systemGray4)], startPoint: .top, endPoint: .bottom))
            Image(systemName: deviceSymbol(kind, os: os)).font(.system(size: size * 0.42, weight: .regular)).foregroundStyle(.primary.opacity(0.8))
        }.frame(width: size, height: size).accessibilityHidden(true)
    }
}
/// Avatar for any contact: an own device shows its device glyph instead of a photo.
struct ContactAvatar: View {
    let friend: Friend
    var size: CGFloat = 52
    var body: some View {
        if friend.ownDevice { DeviceAvatar(kind: friend.deviceKind, os: friend.deviceOs, size: size) }
        else { FriendAvatar(friend: friend, size: size) }
    }
}
func deviceSymbol(_ kind: String?) -> String? {
    switch kind {
    case "phone", "iphone": return "iphone"
    case "tablet", "ipad": return "ipad"
    case "laptop": return "laptopcomputer"
    case "desktop": return "desktopcomputer"
    default: return nil
    }
}
