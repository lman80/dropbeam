import SwiftUI

struct ChatsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var search = ""
    @State private var composing = false
    /// Pinned conversations (local, per device), shown as the avatar grid on top.
    @AppStorage("dropbeam.chat.pinned") private var pinnedRaw = ""
    private var pinnedIDs: [String] { pinnedRaw.split(separator: ",").map(String.init) }
    private func overview(_ friend: Friend) -> ChatOverview? { bridge.chatOverview.first { $0.peerId == friend.id } }
    private func hasThread(_ friend: Friend) -> Bool { overview(friend) != nil || !(bridge.threads[friend.id] ?? []).isEmpty }
    private var conversations: [Friend] {
        bridge.friends.filter { friend in
            hasThread(friend) && (search.isEmpty || ChatRow.title(for: friend).localizedCaseInsensitiveContains(search) ||
                overview(friend)?.lastText?.localizedCaseInsensitiveContains(search) == true)
        }.sorted { (overview($0)?.lastTs ?? 0) > (overview($1)?.lastTs ?? 0) }
    }
    private var pinned: [Friend] {
        guard search.isEmpty else { return [] }
        return pinnedIDs.compactMap { id in bridge.friends.first { $0.id == id } }
    }
    private var rows: [Friend] { search.isEmpty ? conversations.filter { !pinnedIDs.contains($0.id) } : conversations }
    var body: some View {
        NavigationStack(path: $bridge.chatPath) {
            List {
                if !pinned.isEmpty {
                    PinnedGrid(friends: pinned, open: open, unpin: togglePin)
                        .listRowInsets(EdgeInsets(top: 4, leading: 12, bottom: 8, trailing: 12))
                        .listRowSeparator(.hidden)
                }
                ForEach(rows) { friend in
                    Button { open(friend) } label: {
                        ChatRow(friend: friend, overview: overview(friend), unread: (bridge.chatUnread[friend.id] ?? 0) > 0,
                                fallback: bridge.threads[friend.id]?.last?.preview)
                    }
                    .listRowInsets(EdgeInsets(top: 0, leading: 4, bottom: 0, trailing: 16))
                    .listRowSeparator(.hidden, edges: friend.id == rows.first?.id ? .top : [])
                    .swipeActions(edge: .leading, allowsFullSwipe: true) {
                        if (bridge.chatUnread[friend.id] ?? 0) > 0 {
                            Button { markRead(friend) } label: { Label("Read", systemImage: "message.badge.filled.fill") }.tint(ChatPalette.sent)
                        }
                        Button { togglePin(friend) } label: {
                            Label(pinnedIDs.contains(friend.id) ? "Unpin" : "Pin", systemImage: pinnedIDs.contains(friend.id) ? "pin.slash.fill" : "pin.fill")
                        }.tint(.yellow)
                    }
                    .contextMenu { menu(friend) }
                }
            }
            .listStyle(.plain)
            .overlay {
                if conversations.isEmpty && pinned.isEmpty {
                    ContentUnavailableView(search.isEmpty ? "No Conversations" : "No Results",
                        systemImage: search.isEmpty ? "bubble.left.and.bubble.right" : "magnifyingglass",
                        description: Text(search.isEmpty ? "Messages you exchange with friends appear here." : "Try another name or message."))
                        .allowsHitTesting(false)
                }
            }
            .navigationTitle("Chats").navigationBarTitleDisplayMode(.large)
            .searchable(text: $search, prompt: "Search")
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { Haptics.tap(); composing = true } label: { Image(systemName: "square.and.pencil") }
                        .accessibilityLabel("New message")
                }
            }
            .navigationDestination(for: String.self) { id in ConversationView(friendID: id) }
            .sheet(isPresented: $composing) { ChatFriendPicker().environmentObject(bridge) }
        }
        .tint(ChatPalette.sent)
        .onChange(of: bridge.chatPath) { old, new in
            if new.isEmpty, let id = old.last {
                Task { try? await bridge.setTyping(friendId: id, on: false); try? await bridge.closeChat(friendId: id) }
            }
        }
    }
    @ViewBuilder private func menu(_ friend: Friend) -> some View {
        Button { togglePin(friend) } label: {
            Label(pinnedIDs.contains(friend.id) ? "Unpin" : "Pin", systemImage: pinnedIDs.contains(friend.id) ? "pin.slash" : "pin")
        }
        if (bridge.chatUnread[friend.id] ?? 0) > 0 {
            Button { markRead(friend) } label: { Label("Mark as Read", systemImage: "message") }
        }
    }
    private func open(_ friend: Friend) { bridge.perform { try await bridge.openChat(friendId: friend.id) } }
    private func markRead(_ friend: Friend) { bridge.perform { try await bridge.markChatRead(friendId: friend.id) } }
    private func togglePin(_ friend: Friend) {
        Haptics.tap()
        var ids = pinnedIDs
        if let index = ids.firstIndex(of: friend.id) { ids.remove(at: index) } else { ids.append(friend.id) }
        withAnimation(.snappy) { pinnedRaw = ids.joined(separator: ",") }
    }
}

/// One conversation row, Messages style. Title, subtitle and avatar each come from
/// a single helper so per-kind treatments (e.g. the user's own devices) drop in here.
struct ChatRow: View {
    let friend: Friend
    let overview: ChatOverview?
    let unread: Bool
    var fallback: String?
    static func title(for friend: Friend) -> String { friend.name }
    static func subtitle(for friend: Friend, overview: ChatOverview?, fallback: String?) -> String {
        overview?.lastText ?? fallback ?? "No messages yet"
    }
    @ViewBuilder static func avatar(for friend: Friend, size: CGFloat) -> some View { FriendAvatar(friend: friend, size: size) }
    var body: some View {
        HStack(alignment: .center, spacing: 0) {
            Circle().fill(unread ? ChatPalette.sent : .clear).frame(width: 10, height: 10)
                .frame(width: 22).accessibilityHidden(true)
            Self.avatar(for: friend, size: 52).padding(.trailing, 12)
            VStack(alignment: .leading, spacing: 2) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(Self.title(for: friend)).font(.body.weight(.semibold)).foregroundStyle(.primary).lineLimit(1)
                    Spacer(minLength: 4)
                    if let ts = overview?.lastTs {
                        Text(ChatDates.overview(ts)).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary)
                }
                Text(Self.subtitle(for: friend, overview: overview, fallback: fallback))
                    .font(.subheadline).foregroundStyle(.secondary).lineLimit(2, reservesSpace: true)
                    .multilineTextAlignment(.leading)
            }
            .padding(.vertical, 11)
            .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
        }
        .contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityLabel("\(Self.title(for: friend)), \(unread ? "unread, " : "")\(Self.subtitle(for: friend, overview: overview, fallback: fallback))")
    }
}

/// Messages' pinned conversations: large avatars in a grid above the list.
private struct PinnedGrid: View {
    @EnvironmentObject private var bridge: Bridge
    let friends: [Friend]
    let open: (Friend) -> Void
    let unpin: (Friend) -> Void
    var body: some View {
        LazyVGrid(columns: Array(repeating: GridItem(.flexible(), spacing: 8), count: 3), spacing: 14) {
            ForEach(friends) { friend in
                Button { open(friend) } label: {
                    VStack(spacing: 6) {
                        ChatRow.avatar(for: friend, size: 78)
                            .overlay(alignment: .topLeading) {
                                if (bridge.chatUnread[friend.id] ?? 0) > 0 {
                                    Circle().fill(ChatPalette.sent).frame(width: 14, height: 14)
                                        .overlay(Circle().stroke(ChatPalette.background, lineWidth: 2.5)).offset(x: 2, y: 2)
                                }
                            }
                        Text(ChatRow.title(for: friend)).font(.caption).foregroundStyle(.secondary).lineLimit(1)
                    }.frame(maxWidth: .infinity).contentShape(Rectangle())
                }.buttonStyle(.plain)
                    .contextMenu { Button { unpin(friend) } label: { Label("Unpin", systemImage: "pin.slash") } }
                    .accessibilityLabel("\(ChatRow.title(for: friend)), pinned")
            }
        }.padding(.top, 6)
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
                    HStack(spacing: 12) { ChatRow.avatar(for: friend, size: 40); Text(ChatRow.title(for: friend)).font(.body).foregroundStyle(.primary) }
                        .padding(.vertical, 2)
                }
            }.listStyle(.plain).navigationTitle("New Message").navigationBarTitleDisplayMode(.inline)
                .searchable(text: $search, placement: .navigationBarDrawer(displayMode: .always), prompt: "To:")
                .overlay { if bridge.friends.isEmpty { ContentUnavailableView("Add a friend first", systemImage: "person.2") } }
                .toolbar { ToolbarItem(placement: .cancellationAction) { Button("Cancel") { dismiss() } } }
        }.tint(ChatPalette.sent)
    }
}

enum ChatDates {
    /// List timestamps: "9:41 AM", "Yesterday", "Tuesday", "9/12/26".
    static func overview(_ ts: Double) -> String {
        let date = Date(timeIntervalSince1970: ts / 1000), calendar = Calendar.current
        if calendar.isDateInToday(date) { return date.formatted(date: .omitted, time: .shortened) }
        if calendar.isDateInYesterday(date) { return "Yesterday" }
        if let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: Date())).day, days < 7 {
            return date.formatted(.dateTime.weekday(.wide))
        }
        return date.formatted(date: .numeric, time: .omitted)
    }
    /// Thread cluster headers: "**Today** 9:41 AM", "**Tuesday** 9:41 AM", "**Wed, Sep 10** at 3:22 PM".
    static func header(_ date: Date) -> Text {
        let calendar = Calendar.current, time = date.formatted(date: .omitted, time: .shortened)
        let day: String, joiner: String
        if calendar.isDateInToday(date) { day = "Today"; joiner = " " }
        else if calendar.isDateInYesterday(date) { day = "Yesterday"; joiner = " " }
        else if let days = calendar.dateComponents([.day], from: calendar.startOfDay(for: date), to: calendar.startOfDay(for: Date())).day, days < 7 {
            day = date.formatted(.dateTime.weekday(.wide)); joiner = " "
        } else if calendar.isDate(date, equalTo: Date(), toGranularity: .year) {
            day = date.formatted(.dateTime.weekday(.abbreviated).month(.abbreviated).day()); joiner = " at "
        } else {
            day = date.formatted(date: .abbreviated, time: .omitted); joiner = " at "
        }
        return Text(day).fontWeight(.semibold) + Text(joiner + time)
    }
}
