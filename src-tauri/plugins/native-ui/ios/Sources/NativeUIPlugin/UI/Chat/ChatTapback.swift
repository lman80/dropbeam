import SwiftUI
import UIKit

/// A long-pressed message lifted into the Tapback overlay.
struct TapbackTarget: Identifiable {
    let message: ChatMessage
    let lastInRun: Bool
    let frame: CGRect
    var id: String { message.id }
}

/// iMessage long-press: the background blurs, the bubble stays put (nudged to fit),
/// a glass Tapback capsule floats above it and a glass action menu sits below.
struct TapbackOverlay: View {
    @EnvironmentObject private var bridge: Bridge
    let target: TapbackTarget
    var onReply: () -> Void
    var onEdit: () -> Void
    var onClose: () -> Void
    @State private var shown = false
    @State private var moreEmoji = false
    private var message: ChatMessage { target.message }
    private var mine: Bool { message.fromMe }
    private struct Action: Identifiable {
        let title: String, symbol: String
        var destructive = false
        let run: () -> Void
        var id: String { title }
    }
    private var actions: [Action] {
        var list: [Action] = [Action(title: "Reply", symbol: "arrowshape.turn.up.left") { onReply() }]
        let copyText = message.text?.isEmpty == false ? message.text : (message.kind == "file" ? nil : message.preview)
        if let copyText { list.append(Action(title: "Copy", symbol: "doc.on.doc") { UIPasteboard.general.string = copyText }) }
        if mine && message.kind != "file" { list.append(Action(title: "Edit", symbol: "pencil") { onEdit() }) }
        if message.kind == "file" {
            let paths = ChatAttachment.availablePaths(message, bridge: bridge)
            if !paths.isEmpty { list.append(Action(title: "Save or Share", symbol: "square.and.arrow.up") { bridge.perform { try await bridge.shareFiles(paths: paths) } }) }
        }
        if mine {
            list.append(Action(title: "Undo Send", symbol: "arrow.uturn.backward", destructive: true) {
                bridge.perform { try await bridge.deleteMessage(friendId: message.peerId, messageId: message.id) }
            })
        }
        return list
    }
    private var mineReactions: Set<String> { Set((message.reactions ?? []).filter { $0.fromMe == true }.compactMap(\.emoji).map(Tapbacks.key)) }
    private let rowHeight: CGFloat = 46
    private var barSize: CGSize { moreEmoji ? CGSize(width: 324, height: 216) : CGSize(width: 334, height: 52) }
    private var menuHeight: CGFloat { CGFloat(actions.count) * rowHeight + 16 }

    var body: some View {
        GeometryReader { geo in
            let screen = geo.size, insets = Self.windowInsets
            let frame = target.frame
            let badgeRoom: CGFloat = message.reactions?.isEmpty == false ? 26 : 0
            let top = frame.minY - 10 - badgeRoom - barSize.height, bottom = frame.maxY + 10 + menuHeight
            let minY = insets.top + 8, maxY = screen.height - insets.bottom - 8
            let lift: CGFloat = {
                var offset: CGFloat = 0
                if bottom > maxY { offset = maxY - bottom }
                if top + offset < minY { offset = minY - top }
                return offset
            }()
            let dy = shown ? lift : 0
            ZStack(alignment: .topLeading) {
                Rectangle().fill(.thinMaterial).opacity(shown ? 1 : 0)
                    .overlay(Color.black.opacity(shown ? 0.08 : 0))
                    .onTapGesture { close() }
                    .accessibilityLabel("Dismiss").accessibilityAddTraits(.isButton)
                MessageBody(message: message, lastInRun: target.lastInRun)
                    .overlay(alignment: mine ? .topLeading : .topTrailing) {
                        if let reactions = message.reactions, !reactions.isEmpty {
                            TapbackBadges(reactions: reactions, onMine: mine).offset(x: mine ? -22 : 22, y: -25)
                        }
                    }
                    .frame(width: frame.width, height: frame.height)
                    .scaleEffect(shown ? 1.02 : 1, anchor: mine ? .trailing : .leading)
                    .offset(x: frame.minX, y: frame.minY + dy)
                    .allowsHitTesting(false)
                reactionBar
                    .frame(width: barSize.width, height: barSize.height)
                    .scaleEffect(shown ? 1 : 0.4, anchor: mine ? .bottomTrailing : .bottomLeading)
                    .opacity(shown ? 1 : 0)
                    .offset(x: Self.clampX(mine ? frame.maxX - barSize.width : frame.minX, width: barSize.width, screen: screen.width),
                            y: frame.minY + dy - 10 - badgeRoom - barSize.height)
                menu
                    .frame(width: 250)
                    .scaleEffect(shown ? 1 : 0.5, anchor: mine ? .topTrailing : .topLeading)
                    .opacity(shown ? 1 : 0)
                    .offset(x: Self.clampX(mine ? frame.maxX - 250 - MessageBubbleShape.tail : frame.minX + MessageBubbleShape.tail, width: 250, screen: screen.width),
                            y: frame.maxY + dy + 10)
            }
        }
        .ignoresSafeArea()
        .onAppear { withAnimation(.spring(response: 0.34, dampingFraction: 0.78)) { shown = true } }
        .accessibilityAction(.escape) { close() }
    }

    private var reactionBar: some View {
        Group {
            if moreEmoji {
                ScrollView {
                    LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 4), count: 6), spacing: 6) {
                        ForEach(Tapbacks.classic + Tapbacks.more, id: \.self) { emoji in
                            Button { react(emoji) } label: {
                                Text(emoji).font(.system(size: 28)).frame(width: 44, height: 44)
                                    .background(mineReactions.contains(Tapbacks.key(emoji)) ? ChatPalette.sent.opacity(0.25) : .clear, in: Circle())
                            }.buttonStyle(.plain).accessibilityLabel(Tapbacks.name(emoji))
                        }
                    }.padding(12)
                }.scrollIndicators(.hidden)
            } else {
                HStack(spacing: 2) {
                    ForEach(Tapbacks.classic, id: \.self) { emoji in
                        let selected = mineReactions.contains(Tapbacks.key(emoji))
                        Button { react(emoji) } label: {
                            TapbackGlyph(emoji: emoji, size: 19, color: selected ? .white : Color(uiColor: .systemGray))
                                .frame(width: 42, height: 42)
                                .background(selected ? ChatPalette.sent : .clear, in: Circle())
                                .contentShape(Circle())
                        }.buttonStyle(.plain)
                            .accessibilityLabel(Tapbacks.name(emoji)).accessibilityAddTraits(selected ? .isSelected : [])
                    }
                    Button { withAnimation(.spring(response: 0.3, dampingFraction: 0.85)) { moreEmoji = true } } label: {
                        Image(systemName: "face.smiling").font(.system(size: 20, weight: .medium)).foregroundStyle(Color(uiColor: .systemGray))
                            .overlay(alignment: .bottomTrailing) {
                                Image(systemName: "plus.circle.fill").font(.system(size: 10, weight: .bold))
                                    .symbolRenderingMode(.palette).foregroundStyle(.white, Color(uiColor: .systemGray))
                                    .offset(x: 3, y: 3)
                            }
                            .frame(width: 42, height: 42).contentShape(Circle())
                    }.buttonStyle(.plain).accessibilityLabel("More reactions")
                }.padding(.horizontal, 6)
            }
        }
        .modifier(GlassSurface(shape: RoundedRectangle(cornerRadius: moreEmoji ? 28 : 26, style: .continuous)))
    }

    private var menu: some View {
        VStack(spacing: 0) {
            ForEach(Array(actions.enumerated()), id: \.element.id) { index, action in
                if action.destructive && index > 0 {
                    Rectangle().fill(Color.primary.opacity(0.06)).frame(height: 5).padding(.vertical, 4)
                }
                Button { action.run(); close() } label: {
                    HStack(spacing: 14) {
                        Image(systemName: action.symbol).font(.system(size: 17, weight: .regular)).frame(width: 24)
                        Text(action.title).font(.body)
                        Spacer(minLength: 0)
                    }
                    .foregroundStyle(action.destructive ? Color.red : Color.primary)
                    .padding(.horizontal, 18).frame(height: rowHeight).contentShape(Rectangle())
                }.buttonStyle(TapbackRowStyle())
            }
        }
        .padding(.vertical, 8)
        .modifier(GlassSurface(shape: RoundedRectangle(cornerRadius: 26, style: .continuous)))
    }

    private func react(_ emoji: String) {
        Haptics.tap()
        bridge.perform { try await bridge.reactToMessage(friendId: message.peerId, messageId: message.id, emoji: emoji) }
        close()
    }
    private func close() {
        withAnimation(.easeOut(duration: 0.18)) { shown = false }
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.19) { onClose() }
    }
    private static func clampX(_ x: CGFloat, width: CGFloat, screen: CGFloat) -> CGFloat {
        min(max(12, x), max(12, screen - width - 12))
    }
    @MainActor private static var windowInsets: UIEdgeInsets {
        (UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows).first { $0.isKeyWindow })?.safeAreaInsets ?? UIEdgeInsets(top: 59, left: 0, bottom: 34, right: 0)
    }
}

private struct TapbackRowStyle: ButtonStyle {
    func makeBody(configuration: Configuration) -> some View {
        configuration.label.background(Color.primary.opacity(configuration.isPressed ? 0.1 : 0))
    }
}

/// Liquid Glass on iOS 26; a material with a hairline everywhere else.
struct GlassSurface<S: Shape>: ViewModifier {
    let shape: S
    var interactive = false
    func body(content: Content) -> some View {
        if #available(iOS 26, *) {
            content.glassEffect(interactive ? .regular.interactive() : .regular, in: shape)
        } else {
            content.background(.regularMaterial, in: shape)
                .overlay(shape.stroke(Color.primary.opacity(0.08), lineWidth: 0.5))
                .shadow(color: .black.opacity(0.08), radius: 10, y: 4)
        }
    }
}
extension View {
    func glassSurface<S: Shape>(_ shape: S, interactive: Bool = false) -> some View { modifier(GlassSurface(shape: shape, interactive: interactive)) }
}
