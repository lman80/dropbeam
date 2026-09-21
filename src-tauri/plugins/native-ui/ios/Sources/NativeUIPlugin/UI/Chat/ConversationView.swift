import SwiftUI

struct ConversationView: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.scenePhase) private var scenePhase
    let friendID: String
    @State private var reply: ChatMessage?
    @State private var editing: ChatMessage?
    @State private var draft = ""
    @State private var searching = false
    @State private var query = ""
    @State private var matchIndex = 0
    @State private var nearBottom = true
    @State private var unseen = false
    @State private var viewport = CGSize(width: 390, height: 600)
    @State private var loaded = false
    @State private var scrollID: String?
    private var messages: [ChatMessage] { bridge.threads[friendID] ?? [] }
    private var friend: Friend { bridge.friends.first { $0.id == friendID } ?? Friend(id: friendID, name: "Friend") }
    private var matches: [String] {
        guard !query.isEmpty else { return [] }
        return messages.filter { $0.deleted != true && $0.preview.localizedCaseInsensitiveContains(query) }.map(\.id)
    }
    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(messages.enumerated()), id: \.element.id) { index, message in
                        messageRow(message, index: index)
                            .id(message.id)
                    }
                    if bridge.chatTyping[friendID] == true {
                        HStack(alignment: .bottom, spacing: 6) { FriendAvatar(friend: friend, size: 28); TypingBubble(); Spacer() }.padding(.top, 10)
                    }
                    Color.clear.frame(height: 12).id("thread-bottom")
                        .background { GeometryReader { geo in
                            Color.clear.preference(key: ChatBottomKey.self, value: geo.frame(in: .named("thread-scroll")).maxY)
                        } }
                }.scrollTargetLayout().padding(.horizontal, 16).padding(.top, 18)
            }
            .defaultScrollAnchor(.bottom)
            .scrollPosition(id: $scrollID)
            .coordinateSpace(name: "thread-scroll")
            .background { GeometryReader { geo in Color.clear.onAppear { viewport = geo.size }.onChange(of: geo.size) { _, size in viewport = size } } }
            .onPreferenceChange(ChatBottomKey.self) { y in
                nearBottom = y <= viewport.height + 40 && y >= 0
                if nearBottom { unseen = false }
            }
            .scrollDismissesKeyboard(.interactively)
            .overlay(alignment: .bottomTrailing) {
                if unseen && !nearBottom {
                    Button { scrollDown(proxy); unseen = false } label: {
                        Label("New messages", systemImage: "arrow.down").font(.subheadline.weight(.semibold)).padding(12)
                    }.buttonStyle(.plain).chatGlass().padding(16)
                }
            }
            .safeAreaInset(edge: .bottom, spacing: 0) {
                VStack(spacing: 0) {
                    if searching { searchFooter(proxy) }
                    ChatComposer(friendID: friendID, reply: $reply, editing: $editing, text: $draft) { scrollDown(proxy) }
                }
            }
            .onChange(of: messages.map(\.id)) { old, new in
                if !loaded || old.isEmpty || nearBottom || messages.last?.fromMe == true {
                    scrollDown(proxy, animated: loaded); loaded = !new.isEmpty
                } else if new.count > old.count { unseen = true }
            }
            .onChange(of: bridge.chatTyping[friendID]) { _, _ in if nearBottom { scrollDown(proxy) } }
            .onChange(of: viewport.height) { _, _ in if nearBottom { scrollDown(proxy, animated: false) } }
            .onChange(of: matches) { _, _ in matchIndex = 0; scrollToMatch(proxy) }
            .task {
                guard !loaded else { return }
                do {
                    try await bridge.nativeChatFocus(scenePhase == .active)
                    try Task.checkCancellation()
                    try await bridge.chatThread(friendId: friendID)
                    try Task.checkCancellation()
                    await Task.yield()
                    scrollDown(proxy, animated: false)
                    loaded = true
                } catch is CancellationError {} catch { bridge.errorMessage = error.localizedDescription }
            }
        }
        .beamCanvas().navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .tabBar)
        .toolbar {
            ToolbarItem(placement: .principal) {
                VStack(spacing: 2) {
                    FriendAvatar(friend: friend, size: 36)
                    Text(friend.name).font(.caption).lineLimit(1)
                }.accessibilityElement(children: .combine).accessibilityLabel(friend.name)
            }
            ToolbarItem(placement: .topBarTrailing) {
                Button { searching.toggle(); if !searching { query = "" }; Haptics.tap() } label: {
                    Image(systemName: searching ? "xmark" : "magnifyingglass").frame(width: 44, height: 44)
                }.accessibilityLabel(searching ? "Close search" : "Search conversation")
            }
        }
        .modifier(ThreadSearch(enabled: searching, query: $query))
        .onDisappear {
            // Sheets retain this destination; only a pop/switch closes the store.
            if !bridge.chatPath.contains(friendID) || bridge.selectedTab != "chat" {
                Task { try? await bridge.setTyping(friendId: friendID, on: false); try? await bridge.closeChat(friendId: friendID) }
            }
        }
    }
    private func messageRow(_ message: ChatMessage, index: Int) -> some View {
        let previous = index > 0 ? messages[index - 1] : nil
        let next = index + 1 < messages.count ? messages[index + 1] : nil
        let newDay = previous.map { !Calendar.current.isDate($0.date, inSameDayAs: message.date) } ?? true
        let newRun = newDay || previous?.fromMe != message.fromMe || message.ts - (previous?.ts ?? 0) > 300_000
        let lastInRun = next == nil || next?.fromMe != message.fromMe || (next?.ts ?? 0) - message.ts > 300_000 || (next.map { !Calendar.current.isDate($0.date, inSameDayAs: message.date) } ?? true)
        let lastMine = message.fromMe && message.id == messages.last(where: { $0.fromMe })?.id
        return VStack(spacing: 0) {
            if newDay { Text(ChatDates.divider(message.date)).font(.caption2.weight(.semibold)).foregroundStyle(.secondary).padding(.vertical, 18) }
            HStack(alignment: .bottom, spacing: 0) {
                if message.fromMe { Spacer(minLength: 0) }
                if !message.fromMe {
                    Group {
                        if lastInRun { FriendAvatar(friend: friend, size: 28) }
                        else { Color.clear.frame(width: 28, height: 28) }
                    }.padding(.trailing, 6)
                }
                ChatBubble(message: message, lastInRun: lastInRun, query: query,
                    onReply: { reply = message; editing = nil }, onEdit: { editing = message; reply = nil })
                    .frame(maxWidth: max(100, viewport.width * 0.75 - (message.fromMe ? 0 : 34)), alignment: message.fromMe ? .trailing : .leading)
                if !message.fromMe { Spacer(minLength: 0) }
            }.padding(.top, newRun ? 10 : 2)
                .padding(.top, message.reactions?.isEmpty == false ? 18 : 0)
            if lastMine && message.deleted != true, let status = delivery(message) {
                HStack { Spacer(); Text(status).font(.caption2).foregroundStyle(.secondary) }.padding(.top, 4)
            } else if message.edited == true && !message.fromMe {
                HStack { Text("Edited").font(.caption2).foregroundStyle(.secondary); Spacer() }.padding(.top, 4)
            }
        }
    }
    private func delivery(_ message: ChatMessage) -> String? {
        if message.kind == "file", let transfer = bridge.transfers.first(where: { $0.chatOnly == true && $0.id == message.fileXferId }), transfer.state != "completed" { return nil }
        if message.fileXferFailed == true { return nil }
        let status: String
        switch message.status {
        case "read": status = "Read"
        case "delivered", "sent": status = "Delivered"
        case "sending": status = "Sending…"
        case "failed": status = "Not Delivered"
        default: return message.edited == true ? "Edited" : nil
        }
        return status + (message.edited == true ? " · Edited" : "")
    }
    private func scrollDown(_ proxy: ScrollViewProxy, animated: Bool = true) {
        if animated { withAnimation(.easeOut(duration: 0.2)) { proxy.scrollTo("thread-bottom", anchor: .bottom) } }
        else { proxy.scrollTo("thread-bottom", anchor: .bottom) }
    }
    private func scrollToMatch(_ proxy: ScrollViewProxy) {
        guard matches.indices.contains(matchIndex) else { return }
        withAnimation { proxy.scrollTo(matches[matchIndex], anchor: .center) }
    }
    private func searchFooter(_ proxy: ScrollViewProxy) -> some View {
        HStack {
            Text(matches.isEmpty ? "No matches" : "\(matchIndex + 1) of \(matches.count)").font(.caption).foregroundStyle(.secondary)
            Spacer()
            Button { matchIndex = max(0, matchIndex - 1); scrollToMatch(proxy) } label: { Image(systemName: "chevron.up").frame(width: 44, height: 44) }.accessibilityLabel("Previous search result").disabled(matchIndex == 0 || matches.isEmpty)
            Button { matchIndex = min(matches.count - 1, matchIndex + 1); scrollToMatch(proxy) } label: { Image(systemName: "chevron.down").frame(width: 44, height: 44) }.accessibilityLabel("Next search result").disabled(matches.isEmpty || matchIndex >= matches.count - 1)
        }.padding(.horizontal, 20).background(.regularMaterial)
    }
}

private struct ChatBottomKey: PreferenceKey {
    static let defaultValue: CGFloat = .greatestFiniteMagnitude
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = nextValue() }
}
private struct ThreadSearch: ViewModifier {
    let enabled: Bool
    @Binding var query: String
    @ViewBuilder func body(content: Content) -> some View {
        if enabled { content.searchable(text: $query, placement: .navigationBarDrawer(displayMode: .always), prompt: "Search messages") }
        else { content }
    }
}

struct TypingBubble: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    var body: some View {
        TimelineView(.animation(minimumInterval: 0.25, paused: reduceMotion)) { timeline in
            HStack(spacing: 5) {
                ForEach(0..<3) { index in
                    Circle().fill(.secondary).frame(width: 7, height: 7)
                        .opacity(reduceMotion ? 0.65 : 0.35 + 0.65 * max(0, sin(timeline.date.timeIntervalSinceReferenceDate * 4 - Double(index))))
                }
            }.padding(.horizontal, 16).padding(.vertical, 15).background(Color(uiColor: .systemGray5), in: Capsule())
        }.accessibilityLabel("Typing")
    }
}
