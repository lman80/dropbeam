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
    var body: some View {
        Group {
            if let path = friend.avatar, let image = UIImage(contentsOfFile: path.hasPrefix("file://") ? URL(string: path)?.path ?? path : path) {
                Image(uiImage: image).resizable().scaledToFill()
            } else {
                ZStack {
                    LinearGradient(colors: [.beam, .blue.opacity(0.8)], startPoint: .topLeading, endPoint: .bottomTrailing)
                    Text(friend.name.split(separator: " ").prefix(2).compactMap(\.first).map(String.init).joined().uppercased())
                        .font(.system(size: size * 0.33, weight: .semibold)).foregroundStyle(.white)
                }
            }
        }
        .frame(width: size, height: size).clipShape(Circle()).accessibilityHidden(true)
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
func deviceSymbol(_ kind: String?) -> String? {
    switch kind {
    case "phone", "iphone": return "iphone"
    case "tablet", "ipad": return "ipad"
    case "laptop": return "laptopcomputer"
    case "desktop": return "desktopcomputer"
    default: return nil
    }
}
