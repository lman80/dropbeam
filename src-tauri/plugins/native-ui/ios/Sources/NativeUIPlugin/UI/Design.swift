import SwiftUI
import UIKit

// MARK: - DropBeam design language
//
// One system across every screen:
// • Screens are native `List`s (inset grouped) on `BeamBackground` — the system grouped
//   background with a soft beam-coloured glow at the top. Use `.beamList()`.
// • Rows lead with a `RowIcon` (Settings-style tinted tile), an avatar or a `FileGlyph`;
//   navigation rows let the List draw the chevron.
// • One prominent action per screen: `.beamButton(prominent: true)` (Liquid Glass on
//   iOS 26, bordered-prominent before). Everything else is `.beamButton()`.
// • Empty/error states are `ContentUnavailableView`; destructive actions confirm with a
//   `confirmationDialog`; list rows offer swipe actions + context menus for the same actions.
// • Brand tint `.beam` everywhere (Chat keeps its iMessage-blue bubbles).

@MainActor enum Haptics {
    static func tap() { UIImpactFeedbackGenerator(style: .light).impactOccurred() }
    static func success() { UINotificationFeedbackGenerator().notificationOccurred(.success) }
    static func warning() { UINotificationFeedbackGenerator().notificationOccurred(.warning) }
}
extension Color {
    static let beam = Color(uiColor: UIColor { traits in
        traits.userInterfaceStyle == .dark
            ? UIColor(red: 124/255, green: 124/255, blue: 1, alpha: 1)
            : UIColor(red: 91/255, green: 91/255, blue: 240/255, alpha: 1)
    })
}
/// The app's canvas: grouped background + a gentle beam glow drifting at the top.
struct BeamBackground: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.colorScheme) private var scheme
    @State private var drift = false
    var body: some View {
        ZStack(alignment: .top) {
            Color(uiColor: .systemGroupedBackground)
            glow.frame(height: 440)
                .mask(LinearGradient(colors: [.black, .black.opacity(0.55), .clear], startPoint: .top, endPoint: .bottom))
                .opacity(scheme == .dark ? 0.24 : 0.17)
        }
        .ignoresSafeArea()
        .allowsHitTesting(false)
        .accessibilityHidden(true)
        .onAppear { if !reduceMotion { withAnimation(.easeInOut(duration: 9).repeatForever(autoreverses: true)) { drift = true } } }
    }
    @ViewBuilder private var glow: some View {
        if #available(iOS 18, *) {
            MeshGradient(width: 3, height: 3,
                points: [[0,0], [0.5,0], [1,0], [0,0.5], [drift ? 0.65 : 0.35,0.5], [1,0.5], [0,1], [0.5,1], [1,1]],
                colors: [.beam, .blue, .beam, .clear, .beam, .blue, .clear, .clear, .clear])
        } else {
            LinearGradient(colors: [.beam, .blue.opacity(0.6), .clear], startPoint: .topLeading, endPoint: .bottom)
        }
    }
}
/// A floating glass panel for overlays (progress, banners, selection bars) —
/// list content lives in native List sections instead.
struct GlassCard<Content: View>: View {
    @ViewBuilder var content: Content
    var body: some View {
        if #available(iOS 26, *) {
            content.padding(20).frame(maxWidth: .infinity, alignment: .leading)
                .glassEffect(.regular, in: .rect(cornerRadius: 24))
        } else {
            content.padding(20).frame(maxWidth: .infinity, alignment: .leading)
                .background(.regularMaterial, in: RoundedRectangle(cornerRadius: 24, style: .continuous))
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
/// Glass capsule behind small floating text (toasts, banners).
struct GlassCapsule: ViewModifier {
    @ViewBuilder func body(content: Content) -> some View {
        if #available(iOS 26, *) { content.glassEffect(.regular, in: .capsule) }
        else { content.background(.regularMaterial, in: Capsule()) }
    }
}
extension View {
    func beamButton(prominent: Bool = false) -> some View { modifier(BeamButtonStyle(prominent: prominent)) }
    /// Scroll screens that aren't lists (onboarding, scanners).
    func beamCanvas() -> some View { background { BeamBackground() } }
    /// The standard DropBeam screen: an inset-grouped List on the beam canvas.
    func beamList() -> some View {
        listStyle(.insetGrouped).scrollContentBackground(.hidden).background { BeamBackground() }
    }
    func glassCapsule() -> some View { modifier(GlassCapsule()) }
    /// A List row without a cell (heroes, big buttons, headers).
    func clearRow(_ insets: EdgeInsets = EdgeInsets(top: 6, leading: 20, bottom: 6, trailing: 20)) -> some View {
        listRowBackground(Color.clear).listRowInsets(insets).listRowSeparator(.hidden)
    }
}
/// Settings-style tinted tile for a row's leading icon.
struct RowIcon: View {
    let symbol: String
    var color: Color = .beam
    @ScaledMetric(relativeTo: .body) private var size: CGFloat = 30
    var body: some View {
        Image(systemName: symbol).font(.system(size: size * 0.5, weight: .semibold)).foregroundStyle(.white)
            .frame(width: size, height: size)
            .background(color.gradient, in: RoundedRectangle(cornerRadius: size * 0.27, style: .continuous))
            .accessibilityHidden(true)
    }
}
/// A row title with its RowIcon (and optional trailing value). In a NavigationLink
/// the List draws the chevron.
struct RowLabel: View {
    let title: String
    let symbol: String
    var color: Color = .beam
    var value: String? = nil
    var body: some View {
        HStack(spacing: 14) {
            RowIcon(symbol: symbol, color: color)
            Text(title).foregroundStyle(.primary).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
            Spacer(minLength: 8)
            if let value { Text(value).foregroundStyle(.secondary).lineLimit(1) }
        }
        .accessibilityElement(children: .combine)
    }
}
/// A row that opens a web page (privacy policy, support) — marked with ↗.
struct LinkRow: View {
    let title: String
    let symbol: String
    var color: Color = .beam
    let url: URL
    var body: some View {
        Link(destination: url) {
            HStack(spacing: 14) {
                RowIcon(symbol: symbol, color: color)
                Text(title).foregroundStyle(Color.primary).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                Spacer(minLength: 8)
                Image(systemName: "arrow.up.forward").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary)
            }.contentShape(Rectangle())
        }
        .accessibilityHint("Opens in Safari")
    }
}
/// Kept for older call sites (Settings-style row label).
typealias SettingsLinkLabel = RowLabel
/// A row that runs an action (not navigation), with a RowIcon.
struct ActionRow: View {
    let title: String
    let symbol: String
    var color: Color = .beam
    var destructive = false
    let action: () -> Void
    var body: some View {
        Button { Haptics.tap(); action() } label: {
            HStack(spacing: 14) {
                RowIcon(symbol: symbol, color: destructive ? .red : color)
                Text(title).foregroundStyle(destructive ? Color.red : .primary).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                Spacer(minLength: 0)
            }.contentShape(Rectangle())
        }.buttonStyle(.plain)
    }
}
/// A toggle row with a RowIcon.
struct IconToggle: View {
    let title: String
    let symbol: String
    var color: Color = .beam
    @Binding var isOn: Bool
    var body: some View {
        Toggle(isOn: $isOn) {
            HStack(spacing: 14) {
                RowIcon(symbol: symbol, color: color)
                Text(title).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
            }
        }
    }
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
                    Text(initials).font(.system(size: size * 0.36, weight: .semibold, design: .rounded)).foregroundStyle(.white)
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
    private var initials: String {
        let value = friend.name.split(separator: " ").prefix(2).compactMap(\.first).map(String.init).joined().uppercased()
        return value.isEmpty ? "?" : value
    }
}
struct PresenceLabel: View {
    let online: Bool
    var body: some View {
        HStack(spacing: 6) {
            Circle().fill(online ? Color.green : Color(uiColor: .tertiaryLabel)).frame(width: 7, height: 7)
            Text(online ? "Online" : "Offline").font(.subheadline).foregroundStyle(.secondary)
        }.accessibilityElement(children: .combine)
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
/// An own device's avatar: its SF Symbol on a soft disc, like Blip's "Your devices".
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
/// File-type tile: a tinted rounded square with the type's symbol.
struct FileGlyph: View {
    let name: String
    var symbol: String? = nil
    var size: CGFloat = 44
    private var tint: Color { symbol?.hasPrefix("folder") == true ? .blue : .beam }
    var body: some View {
        Image(systemName: symbol ?? Formatters.symbol(name)).font(.system(size: size * 0.44)).foregroundStyle(tint)
            .frame(width: size, height: size)
            .background(tint.opacity(0.13), in: RoundedRectangle(cornerRadius: size * 0.27, style: .continuous))
            .accessibilityHidden(true)
    }
}
/// Centered empty state for non-list screens.
struct BeamEmpty: View {
    let symbol: String
    let title: String
    let detail: String
    var body: some View { ContentUnavailableView(title, systemImage: symbol, description: Text(detail)) }
}
struct BeamError: View {
    let message: String
    let retry: () -> Void
    var body: some View {
        ContentUnavailableView {
            Label("Couldn’t Load This", systemImage: "exclamationmark.triangle")
        } description: { Text(message) } actions: {
            Button("Try Again", action: retry).beamButton()
        }
    }
}
