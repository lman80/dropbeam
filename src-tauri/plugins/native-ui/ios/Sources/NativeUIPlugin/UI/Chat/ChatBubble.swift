import SwiftUI
import UIKit

struct ChatBubble: View {
    @EnvironmentObject private var bridge: Bridge
    let message: ChatMessage
    let lastInRun: Bool
    let query: String
    var onReply: () -> Void
    var onEdit: () -> Void
    private var mine: Bool { message.fromMe }
    private var shape: UnevenRoundedRectangle {
        UnevenRoundedRectangle(topLeadingRadius: 18, bottomLeadingRadius: lastInRun && !mine ? 4 : 18,
            bottomTrailingRadius: lastInRun && mine ? 4 : 18, topTrailingRadius: 18)
    }
    var body: some View {
        bubble
            .overlay(alignment: mine ? .topTrailing : .topLeading) {
                if message.deleted != true, let reactions = message.reactions, !reactions.isEmpty {
                    Text(reactions.compactMap(\.emoji).joined(separator: " ")).font(.caption)
                        .padding(.horizontal, 8).padding(.vertical, 4).chatGlass(radius: 15)
                        .offset(x: mine ? -4 : 4, y: -17)
                        .accessibilityLabel("Reactions: \(reactions.compactMap(\.emoji).joined(separator: ", "))")
                }
            }
            .contextMenu {
                if message.deleted != true {
                    ControlGroup {
                        ForEach(["❤️", "👍", "👎", "😂", "‼️", "❓"], id: \.self) { emoji in
                            Button(emoji) { bridge.perform { try await bridge.reactToMessage(friendId: message.peerId, messageId: message.id, emoji: emoji) } }
                        }
                    }
                    Button(action: onReply) { Label("Reply", systemImage: "arrowshape.turn.up.left") }
                    Button { UIPasteboard.general.string = message.text?.isEmpty == false ? message.text : message.preview } label: { Label("Copy", systemImage: "doc.on.doc") }
                    if mine && message.kind != "file" {
                        Button(action: onEdit) { Label("Edit", systemImage: "pencil") }
                    }
                    if mine {
                        Button(role: .destructive) { bridge.perform { try await bridge.deleteMessage(friendId: message.peerId, messageId: message.id) } } label: { Label("Unsend", systemImage: "arrow.uturn.backward") }
                    }
                    if message.kind == "file" {
                        let paths = ChatAttachment.availablePaths(message, bridge: bridge)
                        if !paths.isEmpty {
                            Button { bridge.perform { try await bridge.shareFiles(paths: paths) } } label: { Label("Save / Share", systemImage: "square.and.arrow.up") }
                        }
                    }
                }
            } preview: {
                bubble.frame(maxWidth: 280).padding(12).onAppear { Haptics.tap() }
            }
    }
    @ViewBuilder private var bubble: some View {
        if message.deleted == true {
            Text("Message deleted").italic().font(.body).foregroundStyle(.secondary)
                .padding(.vertical, 8).padding(.horizontal, 12).overlay(shape.stroke(.secondary.opacity(0.3), lineWidth: 1))
        } else if message.kind == "file" {
            VStack(alignment: mine ? .trailing : .leading, spacing: 4) {
                if let caption = message.text, !caption.isEmpty {
                    highlighted(caption).padding(.horizontal, 12).padding(.vertical, 8)
                        .foregroundStyle(mine ? Color.white : Color.primary)
                        .background(mine ? Color.beam : Color(uiColor: .systemGray5), in: shape)
                }
                ChatAttachment(message: message)
            }
        } else {
            VStack(alignment: .leading, spacing: 4) {
                if let quote = message.replyPreview, !quote.isEmpty {
                    HStack(spacing: 6) {
                        RoundedRectangle(cornerRadius: 1).fill(mine ? Color.white.opacity(0.7) : .beam).frame(width: 2)
                        Text(quote).font(.subheadline).lineLimit(3)
                    }.padding(8).background(mine ? Color.white.opacity(0.16) : Color.primary.opacity(0.06), in: RoundedRectangle(cornerRadius: 10))
                        .padding([.horizontal, .top], 6)
                }
                highlighted(message.text ?? "").padding(.vertical, 8).padding(.horizontal, 12)
            }
            .foregroundStyle(mine ? Color.white : Color.primary)
            .background {
                if mine { LinearGradient(colors: [.beam.opacity(0.88), .beam], startPoint: .top, endPoint: .bottom) }
                else { Color(uiColor: .systemGray5) }
            }.clipShape(shape)
        }
    }
    private func highlighted(_ text: String) -> Text {
        var value = AttributedString(text)
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
        return Text(value).font(.system(size: 17))
    }
}
