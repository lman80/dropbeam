import SwiftUI

struct ChatsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var search = ""
    @State private var composing = false
    private var conversations: [Friend] {
        bridge.friends.filter { friend in
            let overview = bridge.chatOverview.first { $0.peerId == friend.id }
            return (overview != nil || !(bridge.threads[friend.id] ?? []).isEmpty) &&
                (search.isEmpty || friend.name.localizedCaseInsensitiveContains(search) ||
                 overview?.lastText?.localizedCaseInsensitiveContains(search) == true)
        }.sorted { lhs, rhs in
            (bridge.chatOverview.first { $0.peerId == lhs.id }?.lastTs ?? 0) >
            (bridge.chatOverview.first { $0.peerId == rhs.id }?.lastTs ?? 0)
        }
    }
    var body: some View {
        NavigationStack(path: $bridge.chatPath) {
            List {
                Section {
                    ForEach(conversations) { friend in
                        Button { bridge.perform { try await bridge.openChat(friendId: friend.id) } } label: {
                            row(friend)
                        }
                        .buttonStyle(.plain)
                        .listRowBackground(ChatListSurface())
                        .swipeActions(edge: .leading) {
                            if (bridge.chatUnread[friend.id] ?? 0) > 0 {
                                Button { bridge.perform { try await bridge.markChatRead(friendId: friend.id) } } label: {
                                    Label("Mark as Read", systemImage: "envelope.open")
                                }.tint(.beam)
                            }
                        }
                    }
                }
            }
            .listStyle(.insetGrouped).scrollContentBackground(.hidden)
            .contentMargins(.bottom, 24, for: .scrollContent)
            .overlay {
                if conversations.isEmpty {
                    ContentUnavailableView(search.isEmpty ? "Say hello." : "No conversations found",
                        systemImage: search.isEmpty ? "bubble.left.and.bubble.right" : "magnifyingglass",
                        description: Text(search.isEmpty ? "Start a conversation with someone close, wherever they are." : "Try another name or message."))
                        .allowsHitTesting(false)
                }
            }
            .navigationTitle("Chat").navigationBarTitleDisplayMode(.large).beamCanvas()
            .searchable(text: $search, prompt: "Names and messages")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { Haptics.tap(); composing = true } label: {
                        Image(systemName: "square.and.pencil").frame(minWidth: 28, minHeight: 32)
                    }.beamButton().accessibilityLabel("New message")
                }
            }
            .navigationDestination(for: String.self) { id in ConversationView(friendID: id) }
            .sheet(isPresented: $composing) { ChatFriendPicker().environmentObject(bridge) }
        }
        .onChange(of: bridge.chatPath) { old, new in
            if new.isEmpty, let id = old.last {
                Task { try? await bridge.setTyping(friendId: id, on: false); try? await bridge.closeChat(friendId: id) }
            }
        }
    }
    private func row(_ friend: Friend) -> some View {
        let overview = bridge.chatOverview.first { $0.peerId == friend.id }
        let unread = (bridge.chatUnread[friend.id] ?? 0) > 0
        return HStack(spacing: 12) {
            Circle().fill(unread ? Color.blue : .clear).frame(width: 7, height: 7).accessibilityHidden(true)
            FriendAvatar(friend: friend, size: 52)
            VStack(alignment: .leading, spacing: 5) {
                HStack(alignment: .firstTextBaseline) {
                    Text(friend.name).font(.headline).fontWeight(unread ? .bold : .semibold).foregroundStyle(.primary)
                    Spacer(minLength: 2)
                    if let ts = overview?.lastTs {
                        Text(ChatDates.overview(ts)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                Text(overview?.lastText ?? bridge.threads[friend.id]?.last?.preview ?? "New conversation")
                    .font(.subheadline).foregroundStyle(.secondary).lineLimit(2)
            }
        }.padding(.vertical, 8).frame(minHeight: 68)
            .accessibilityElement(children: .combine)
            .accessibilityLabel("\(friend.name), \(unread ? "unread, " : "")\(overview?.lastText ?? "New conversation")")
    }
}

private struct ChatListSurface: View {
    var body: some View {
        if #available(iOS 26, *) { Color.clear.glassEffect(.regular, in: .rect) }
        else { Rectangle().fill(.ultraThinMaterial) }
    }
}

private struct ChatFriendPicker: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    @State private var search = ""
    var body: some View {
        NavigationStack {
            List(bridge.friends.filter { search.isEmpty || $0.name.localizedCaseInsensitiveContains(search) }) { friend in
                Button {
                    dismiss()
                    bridge.perform { try await bridge.openChat(friendId: friend.id) }
                } label: {
                    HStack(spacing: 14) { FriendAvatar(friend: friend); Text(friend.name).font(.headline).foregroundStyle(.primary) }
                        .padding(.vertical, 6)
                }.listRowBackground(Color.clear)
            }.scrollContentBackground(.hidden).beamCanvas().navigationTitle("New Message")
                .searchable(text: $search, prompt: "Choose a friend")
                .overlay { if bridge.friends.isEmpty { ContentUnavailableView("Add a friend first", systemImage: "person.2") } }
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
        }.tint(.beam)
    }
}

enum ChatDates {
    static func overview(_ ts: Double) -> String {
        let date = Date(timeIntervalSince1970: ts / 1000), calendar = Calendar.current
        if calendar.isDateInToday(date) { return date.formatted(date: .omitted, time: .shortened) }
        if calendar.isDate(date, equalTo: Date(), toGranularity: .weekOfYear) { return date.formatted(.dateTime.weekday(.abbreviated)) }
        return date.formatted(date: .numeric, time: .omitted)
    }
    static func divider(_ date: Date) -> String {
        let day = Calendar.current.isDateInToday(date) ? "Today" : Calendar.current.isDateInYesterday(date) ? "Yesterday" : date.formatted(date: .abbreviated, time: .omitted)
        return "\(day) \(date.formatted(date: .omitted, time: .shortened))"
    }
}

extension View {
    @ViewBuilder func chatGlass(radius: CGFloat = 22) -> some View {
        if #available(iOS 26, *) { glassEffect(.regular, in: .rect(cornerRadius: radius)) }
        else { background(.ultraThinMaterial, in: RoundedRectangle(cornerRadius: radius)) }
    }
}
