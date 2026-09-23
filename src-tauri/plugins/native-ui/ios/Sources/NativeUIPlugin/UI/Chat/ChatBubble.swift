import SwiftUI
import UIKit

/// iMessage palette: flat system-blue sent bubbles, soft gray received bubbles.
enum ChatPalette {
    static let sent = Color(uiColor: UIColor(red: 10/255, green: 132/255, blue: 1, alpha: 1))
    static let received = Color(uiColor: UIColor { $0.userInterfaceStyle == .dark
        ? UIColor(red: 38/255, green: 38/255, blue: 41/255, alpha: 1)
        : UIColor(red: 233/255, green: 233/255, blue: 235/255, alpha: 1) })
    static let background = Color(uiColor: .systemBackground)
    static func fill(_ mine: Bool) -> Color { mine ? sent : received }
}

/// The classic iMessage bubble: a continuous rounded body plus the curled tail on
/// the last bubble of a run. Every bubble reserves the tail's width so edges align.
struct MessageBubbleShape: Shape {
    static let tail: CGFloat = 6
    var mine: Bool
    var tailed: Bool
    func path(in rect: CGRect) -> Path {
        let t = Self.tail, w = rect.width, h = rect.height, bw = max(0, w - t)
        let r = min(18, h / 2, bw / 2)
        var p = Path()
        p.move(to: CGPoint(x: r, y: 0))
        p.addLine(to: CGPoint(x: bw - r, y: 0))
        p.addArc(tangent1End: CGPoint(x: bw, y: 0), tangent2End: CGPoint(x: bw, y: r), radius: r)
        if tailed {
            p.addLine(to: CGPoint(x: bw, y: max(r, h - 14)))
            p.addCurve(to: CGPoint(x: w, y: h), control1: CGPoint(x: bw, y: h - 3), control2: CGPoint(x: w - 1.5, y: h))
            p.addCurve(to: CGPoint(x: bw - 6.5, y: h - 3.5), control1: CGPoint(x: w - 4.5, y: h + 0.5), control2: CGPoint(x: bw - 3, y: h - 1.2))
            p.addCurve(to: CGPoint(x: max(r, bw - 16), y: h), control1: CGPoint(x: bw - 9.5, y: h - 0.4), control2: CGPoint(x: bw - 12.5, y: h))
        } else {
            p.addLine(to: CGPoint(x: bw, y: h - r))
            p.addArc(tangent1End: CGPoint(x: bw, y: h), tangent2End: CGPoint(x: bw - r, y: h), radius: r)
        }
        p.addLine(to: CGPoint(x: r, y: h))
        p.addArc(tangent1End: CGPoint(x: 0, y: h), tangent2End: CGPoint(x: 0, y: h - r), radius: r)
        p.addLine(to: CGPoint(x: 0, y: r))
        p.addArc(tangent1End: CGPoint(x: 0, y: 0), tangent2End: CGPoint(x: r, y: 0), radius: r)
        p.closeSubpath()
        let flip = mine ? CGAffineTransform.identity : CGAffineTransform(a: -1, b: 0, c: 0, d: 1, tx: w, ty: 0)
        return p.applying(flip.concatenating(CGAffineTransform(translationX: rect.minX, y: rect.minY)))
    }
}

/// Tapbacks: the six classic reactions render as glyphs (like Messages); anything
/// else a peer sends renders as its emoji.
enum Tapbacks {
    static let classic = ["❤️", "👍", "👎", "😂", "‼️", "❓"]
    static let more = ["😍", "🔥", "🎉", "🙏", "😮", "😢", "😡", "👏", "💯", "✅", "👀", "🤔", "😅", "🥹", "🙌", "💪", "😎", "🤝", "✨", "😭", "🤣", "💀", "🫶", "👌"]
    static func key(_ emoji: String) -> String { emoji.replacingOccurrences(of: "\u{FE0F}", with: "") }
    static func symbol(_ emoji: String) -> String? {
        switch key(emoji) {
        case "❤": return "heart.fill"
        case "👍": return "hand.thumbsup.fill"
        case "👎": return "hand.thumbsdown.fill"
        case "‼": return "exclamationmark.2"
        case "❓": return "questionmark"
        default: return nil
        }
    }
    static func name(_ emoji: String) -> String {
        switch key(emoji) {
        case "❤": return "Heart"
        case "👍": return "Thumbs up"
        case "👎": return "Thumbs down"
        case "😂": return "Ha ha"
        case "‼": return "Exclamation"
        case "❓": return "Question"
        default: return emoji
        }
    }
}

struct TapbackGlyph: View {
    let emoji: String
    var size: CGFloat = 15
    var color: Color = .secondary
    var body: some View {
        if let symbol = Tapbacks.symbol(emoji) {
            Image(systemName: symbol).font(.system(size: size, weight: .bold)).foregroundStyle(color)
        } else if Tapbacks.key(emoji) == "😂" {
            VStack(spacing: -size * 0.2) { Text("HA"); Text("HA") }
                .font(.system(size: size * 0.55, weight: .black, design: .rounded)).foregroundStyle(color)
        } else {
            Text(emoji).font(.system(size: size * 1.1))
        }
    }
}

/// Reaction badges pinned to a bubble's top corner, iMessage style.
struct TapbackBadges: View {
    let reactions: [ChatReaction]
    /// Whether the reacted-to message is mine (badges then sit on its leading corner).
    let onMine: Bool
    private var unique: [ChatReaction] {
        var seen = Set<String>()
        return reactions.filter { r in
            guard let emoji = r.emoji, !emoji.isEmpty else { return false }
            return seen.insert("\(r.fromMe == true)|\(Tapbacks.key(emoji))").inserted
        }
    }
    var body: some View {
        let items = Array(unique.suffix(3))
        HStack(spacing: -6) {
            ForEach(Array(items.enumerated()), id: \.offset) { index, reaction in
                badge(reaction, tailed: index == (onMine ? items.count - 1 : 0))
            }
        }
        .accessibilityElement(children: .ignore)
        .accessibilityLabel("Reactions: " + unique.compactMap { r in r.emoji.map { "\(Tapbacks.name($0))\(r.fromMe == true ? " from you" : "")" } }.joined(separator: ", "))
    }
    private func badge(_ reaction: ChatReaction, tailed: Bool) -> some View {
        let fromMe = reaction.fromMe == true, emoji = reaction.emoji ?? ""
        let fill = fromMe ? ChatPalette.sent : ChatPalette.received
        let glyph: Color = fromMe ? .white : (Tapbacks.key(emoji) == "❤" ? Color(uiColor: .systemPink) : Color(uiColor: .systemGray))
        return ZStack {
            if tailed {
                Circle().fill(fill).frame(width: 9, height: 9)
                    .overlay(Circle().stroke(ChatPalette.background, lineWidth: 1.5))
                    .offset(x: onMine ? 12 : -12, y: 13)
                Circle().fill(fill).frame(width: 4.5, height: 4.5).offset(x: onMine ? 16 : -16, y: 19)
            }
            Circle().fill(fill).frame(width: 32, height: 32)
                .overlay(Circle().stroke(ChatPalette.background, lineWidth: 2))
            TapbackGlyph(emoji: emoji, size: 14, color: glyph)
        }.frame(width: 32, height: 32)
    }
}

/// One message's visual body (bubble, media, caption, reply quote). Used both in
/// the thread and, identically, for the lifted copy in the Tapback overlay.
struct MessageBody: View {
    let message: ChatMessage
    let lastInRun: Bool
    var query = ""
    private var mine: Bool { message.fromMe }
    var body: some View {
        if message.kind == "file" {
            VStack(alignment: mine ? .trailing : .leading, spacing: 2) {
                ChatAttachment(message: message)
                    .padding(mine ? .trailing : .leading, MessageBubbleShape.tail)
                if let caption = message.text, !caption.isEmpty { textBubble(caption, tailed: lastInRun) }
            }
        } else if let text = message.text, message.replyPreview?.isEmpty != false, Self.isJumbo(text) {
            Text(text).font(.system(size: 48)).padding(.horizontal, MessageBubbleShape.tail + 2)
                .accessibilityLabel(text)
        } else {
            VStack(alignment: mine ? .trailing : .leading, spacing: 3) {
                if let quote = message.replyPreview, !quote.isEmpty {
                    HStack(alignment: .firstTextBaseline, spacing: 6) {
                        Image(systemName: "arrowshape.turn.up.left.fill").font(.caption2).foregroundStyle(.tertiary)
                        Text(quote).font(.subheadline).lineLimit(2).foregroundStyle(.secondary)
                    }
                    .padding(.vertical, 6).padding(.horizontal, 11)
                    .overlay(RoundedRectangle(cornerRadius: 16, style: .continuous).strokeBorder(Color.secondary.opacity(0.3), lineWidth: 1))
                    .padding(mine ? .trailing : .leading, MessageBubbleShape.tail)
                    .accessibilityLabel("In reply to: \(quote)")
                }
                textBubble(message.text ?? "", tailed: lastInRun)
            }
        }
    }
    private func textBubble(_ text: String, tailed: Bool) -> some View {
        Text(Self.attributed(text, query: query))
            .font(.body)
            .foregroundStyle(mine ? Color.white : Color.primary)
            .tint(mine ? .white : ChatPalette.sent)
            .fixedSize(horizontal: false, vertical: true)
            .padding(.vertical, 7).padding(.horizontal, 12)
            .padding(mine ? .trailing : .leading, MessageBubbleShape.tail)
            .background(ChatPalette.fill(mine), in: MessageBubbleShape(mine: mine, tailed: tailed))
            .contentShape(MessageBubbleShape(mine: mine, tailed: tailed))
    }
    /// 1–3 emoji and nothing else render large without a bubble, like Messages.
    static func isJumbo(_ text: String) -> Bool {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed.count <= 3 else { return false }
        return trimmed.allSatisfy { ch in
            guard let first = ch.unicodeScalars.first else { return false }
            return first.properties.isEmojiPresentation || (ch.unicodeScalars.count > 1 && first.properties.isEmoji)
        }
    }
    private static let linkDetector = try? NSDataDetector(types: NSTextCheckingResult.CheckingType.link.rawValue)
    static func attributed(_ text: String, query: String) -> AttributedString {
        var value = AttributedString(text)
        for match in linkDetector?.matches(in: text, range: NSRange(text.startIndex..., in: text)) ?? [] {
            guard let url = match.url, let range = Range(match.range, in: text),
                  let lower = AttributedString.Index(range.lowerBound, within: value),
                  let upper = AttributedString.Index(range.upperBound, within: value) else { continue }
            value[lower..<upper].link = url
            value[lower..<upper].underlineStyle = .single
        }
        if !query.isEmpty {
            var start = text.startIndex
            while start < text.endIndex, let range = text.range(of: query, options: [.caseInsensitive, .diacriticInsensitive], range: start..<text.endIndex) {
                if let lower = AttributedString.Index(range.lowerBound, within: value), let upper = AttributedString.Index(range.upperBound, within: value) {
                    value[lower..<upper].backgroundColor = .yellow
                    value[lower..<upper].foregroundColor = .black
                }
                start = range.upperBound
            }
        }
        return value
    }
}

/// A message in the thread: body + Tapback badges; long-press lifts it into the
/// Tapback overlay (reactions + actions live there, never always-visible glyphs).
struct ChatBubble: View {
    @EnvironmentObject private var bridge: Bridge
    let message: ChatMessage
    let lastInRun: Bool
    let query: String
    var lifted = false
    var onReply: () -> Void
    var onEdit: () -> Void
    var onLongPress: (CGRect) -> Void
    /// Written on every layout pass but never observed, so scrolling does not re-render bubbles.
    @State private var frame = FrameBox()
    private var mine: Bool { message.fromMe }
    var body: some View {
        MessageBody(message: message, lastInRun: lastInRun, query: query)
            .overlay(alignment: mine ? .topLeading : .topTrailing) { badges }
            .onGeometryChange(for: CGRect.self) { $0.frame(in: .global) } action: { frame.rect = $0 }
            .opacity(lifted ? 0 : 1)
            .onLongPressGesture(minimumDuration: 0.32) {
                UIImpactFeedbackGenerator(style: .medium).impactOccurred()
                onLongPress(frame.rect)
            }
            .accessibilityElement(children: .combine)
            .accessibilityActions {
                ForEach(Tapbacks.classic, id: \.self) { emoji in
                    Button("React \(Tapbacks.name(emoji))") { bridge.perform { try await bridge.reactToMessage(friendId: message.peerId, messageId: message.id, emoji: emoji) } }
                }
                Button("Reply", action: onReply)
                if mine && message.kind != "file" { Button("Edit", action: onEdit) }
                if mine { Button("Undo Send") { bridge.perform { try await bridge.deleteMessage(friendId: message.peerId, messageId: message.id) } } }
            }
    }
    @ViewBuilder private var badges: some View {
        if let reactions = message.reactions, !reactions.isEmpty {
            TapbackBadges(reactions: reactions, onMine: mine)
                .offset(x: mine ? -22 : 22, y: -25)
        }
    }
}

final class FrameBox { var rect: CGRect = .zero }
