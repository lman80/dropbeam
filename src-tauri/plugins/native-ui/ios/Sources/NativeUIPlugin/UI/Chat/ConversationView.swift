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
    @State private var tapback: TapbackTarget?
    @State private var showDetail = false
    @State private var blocking: Friend?
    @State private var reporting: ReportTarget?
    /// Report chosen in the Tapback menu: shown once the cover has closed.
    @State private var pendingReport: ReportTarget?
    /// Points the composer covers from the screen bottom (feedback button keeps clear).
    @State private var composerHeight: CGFloat = 0
    /// An active check of the friend is in flight (header reads "Connecting…").
    @State private var probing = true
    /// The latest active check got no answer, overriding a still-fresh presence stamp.
    @State private var probedOffline = false
    @State private var lastProbe = Date.distantPast
    /// Bumped to restart the check loop (thread opened again / app foregrounded).
    @State private var probeRun = 0
    /// Reachable right now, as far as the latest evidence says.
    private var online: Bool { bridge.presence[friendID] == true && !probedOffline }
    /// Offline, and the check that says so has finished.
    private var knownOffline: Bool { !online && !probing }
    private var hasQueued: Bool { messages.contains { $0.fromMe && $0.deleted != true && ["sending", "failed"].contains($0.status ?? "") } }
    /// A friend (not one of the user's own devices): Report / Block are offered.
    private var reportable: Bool {
        guard let f = bridge.friends.first(where: { $0.id == friendID }), !f.ownDevice else { return false }
        guard let account = bridge.myDevice?.accountPub, !account.isEmpty else { return true }
        return f.accountPub != account
    }
    private var messages: [ChatMessage] { bridge.threads[friendID] ?? [] }
    /// Where a message to them would wait while they're offline (a Transfer Server's name).
    private var holdOn: String? { bridge.holdRoutes[friendID] }
    private var firstName: String { ServerCopy.firstName(friend.displayName) }
    /// A Transfer Server this person shared that hasn't been answered yet.
    private var offer: UsableServer? {
        let endpoints = Set(bridge.friends.filter { $0.id == friendID || $0.groupedUnder == friendID }.compactMap(\.endpointId))
        return bridge.servers.first { endpoints.contains($0.eid) && $0.offer == "new" && !$0.revoked && !$0.own }
    }
    private var friend: Friend { bridge.friends.first { $0.id == friendID } ?? Friend(id: friendID, name: "Friend") }
    private var matches: [String] {
        guard !query.isEmpty else { return [] }
        return messages.filter { $0.deleted != true && $0.preview.localizedCaseInsensitiveContains(query) }.map(\.id)
    }
    /// Messages starts a new time cluster after an hour of quiet or on a new day.
    private static let clusterGap: Double = 3_600_000
    /// Consecutive bubbles from one sender group tightly within this window.
    private static let runGap: Double = 300_000

    var body: some View {
        ScrollViewReader { proxy in
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(Array(messages.enumerated()), id: \.element.id) { index, message in
                        messageRow(message, index: index)
                            .id(message.id)
                    }
                    if let offer {
                        ServerOfferCard(server: offer, friendName: ServerCopy.firstName(friend.displayName))
                            .padding(.top, 16).padding(.horizontal, 4)
                            .transition(.opacity)
                    }
                    if bridge.chatTyping[friendID] == true {
                        HStack(alignment: .bottom, spacing: 6) { ContactAvatar(friend: friend, size: 28); TypingBubble(); Spacer() }
                            .padding(.top, 10).transition(.scale(scale: 0.6, anchor: .bottomLeading).combined(with: .opacity))
                    }
                    Color.clear.frame(height: 10).id("thread-bottom")
                        .background { GeometryReader { geo in
                            Color.clear.preference(key: ChatBottomKey.self, value: geo.frame(in: .named("thread-scroll")).maxY)
                        } }
                }.scrollTargetLayout().padding(.horizontal, 12).padding(.top, 8)
                    .animation(.spring(response: 0.32, dampingFraction: 0.86), value: bridge.chatTyping[friendID] == true)
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
            .overlay(alignment: .bottom) {
                if unseen && !nearBottom {
                    Button { scrollDown(proxy); unseen = false } label: {
                        Label("New Messages", systemImage: "arrow.down").font(.subheadline.weight(.semibold))
                            .padding(.horizontal, 14).padding(.vertical, 9)
                    }.buttonStyle(.plain).glassSurface(Capsule(), interactive: true).padding(.bottom, 10)
                        .transition(.move(edge: .bottom).combined(with: .opacity))
                }
            }
            .modifier(ComposerBar {
                VStack(spacing: 0) {
                    if searching { searchFooter(proxy) }
                    if knownOffline && !searching { offlineNote.transition(.opacity) }
                    ChatComposer(friendID: friendID, reply: $reply, editing: $editing, text: $draft) { scrollDown(proxy) }
                }
                // The feedback button stays available here, parked above the composer
                // (it grows with replies, drafts and attachments) — never over Send.
                .animation(.easeInOut(duration: 0.2), value: knownOffline)
                .background { GeometryReader { geo in
                    Color.clear.preference(key: ComposerTopKey.self, value: geo.frame(in: .global).minY)
                } }
            })
            .onPreferenceChange(ComposerTopKey.self) { top in
                let screen = (UIApplication.shared.connectedScenes.first as? UIWindowScene)?.screen.bounds.height ?? 0
                if top.isFinite, screen > 0 { composerHeight = max(0, screen - top); SuperFeedback.setBottomObstruction(composerHeight) }
            }
            .onAppear { if composerHeight > 0 { SuperFeedback.setBottomObstruction(composerHeight) } }
            .onChange(of: messages.map(\.id)) { old, new in
                if !loaded || old.isEmpty || nearBottom || messages.last?.fromMe == true {
                    scrollDown(proxy, animated: loaded); loaded = !new.isEmpty
                } else if new.count > old.count { withAnimation { unseen = true } }
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
        // Presence: check the friend the moment the thread opens (and again on
        // return to the foreground) instead of waiting on the background beacon.
        .task(id: probeRun) { await presenceLoop() }
        .task(id: probeRun) { await bridge.refreshHoldRoute(friendId: friendID) }
        .onChange(of: bridge.servers) { _, _ in Task { await bridge.refreshHoldRoute(friendId: friendID) } }
        .onReceive(NotificationCenter.default.publisher(for: Notification.Name("DropBeam.mailbox://servers"))) { _ in
            Task { await bridge.refreshHoldRoute(friendId: friendID) }
        }
        .onChange(of: scenePhase) { _, phase in
            if phase == .active, Date().timeIntervalSince(lastProbe) > 5 { probeRun += 1 }
        }
        .onChange(of: bridge.presence[friendID] == true) { _, isOnline in
            guard isOnline else { return }
            probedOffline = false; probing = false
            // Seen through another channel (a folder beacon, an incoming connection):
            // one quick check makes the engine flush what was waiting for them.
            if hasQueued { Task { _ = try? await bridge.checkPresence(id: friendID) } }
        }
        .onChange(of: bridge.chatTyping[friendID] == true) { _, typing in if typing { probedOffline = false; probing = false } }
        .onChange(of: messages.last?.id) { _, _ in
            // Their new message is proof they're reachable right now.
            if let last = messages.last, !last.fromMe, Date().timeIntervalSince(last.date) < 60 { probedOffline = false; probing = false }
        }
        .background(ChatPalette.background.ignoresSafeArea())
        .navigationBarTitleDisplayMode(.inline)
        .toolbar(.hidden, for: .tabBar)
        .toolbar {
            ToolbarItem(placement: .principal) { header }
            // One trailing item, as wide as Back, so the contact header sits dead centre.
            ToolbarItem(placement: .topBarTrailing) {
                if searching {
                    Button { searching = false; query = ""; Haptics.tap() } label: { Image(systemName: "xmark") }
                        .accessibilityLabel("Close search")
                } else {
                    Menu {
                        Button("Search", systemImage: "magnifyingglass") { searching = true }
                        Button("Contact Info", systemImage: "person.crop.circle") { showDetail = true }
                        if reportable {
                            Divider()
                            Button("Report \(friend.displayName)…", systemImage: "exclamationmark.bubble") { reporting = ReportTarget(friend: friend) }
                            Button("Block \(friend.displayName)…", systemImage: "hand.raised", role: .destructive) { blocking = friend }
                        }
                    } label: { Image(systemName: "ellipsis") }.accessibilityLabel("More")
                }
            }
        }
        .modifier(ThreadSearch(enabled: searching, query: $query))
        .fullScreenCover(item: Binding(get: { tapback }, set: { value in instant { tapback = value } })) { target in
            TapbackOverlay(target: target,
                onReply: { reply = target.message; editing = nil },
                onEdit: { editing = target.message; reply = nil },
                onReport: reportable ? { pendingReport = ReportTarget(friend: friend, message: target.message) } : nil,
                onClose: {
                    instant { tapback = nil }
                    if let next = pendingReport {
                        pendingReport = nil
                        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { reporting = next }
                    }
                })
                .environmentObject(bridge)
                .presentationBackground(.clear)
        }
        .safetyPrompts(block: $blocking, report: $reporting)
        .sheet(isPresented: $showDetail) {
            NavigationStack {
                FriendDetailView(friendID: friendID, initial: friend)
                    .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { showDetail = false } } }
            }.environmentObject(bridge).tint(.beam)
        }
        .onDisappear {
            SuperFeedback.setBottomObstruction(0)
            // Sheets retain this destination; only a pop/switch closes the store.
            if !bridge.chatPath.contains(friendID) || bridge.selectedTab != "chat" {
                Task { try? await bridge.setTyping(friendId: friendID, on: false); try? await bridge.closeChat(friendId: friendID) }
            }
        }
    }

    /// Avatar over the name capsule, centered in the glass navigation bar.
    private var header: some View {
        Button { Haptics.tap(); showDetail = true } label: {
            VStack(spacing: -4) {
                ContactAvatar(friend: friend, size: 36).zIndex(1)
                // iOS 26 Messages: the name rides a glass capsule so it stays legible
                // over bubbles scrolling beneath the bar.
                VStack(spacing: 0) {
                    HStack(spacing: 3) {
                        Text(friend.displayName).font(.caption.weight(.semibold)).lineLimit(1)
                        Image(systemName: "chevron.right").font(.system(size: 8, weight: .bold)).foregroundStyle(.secondary)
                    }
                    .foregroundStyle(Color.primary)
                    TimelineView(.periodic(from: .now, by: 60)) { _ in
                        Text(presenceText).font(.caption2).foregroundStyle(online ? Color.green : Color.secondary)
                            .lineLimit(1).contentTransition(.opacity)
                    }
                }
                .dynamicTypeSize(...DynamicTypeSize.xxLarge)
                .padding(.horizontal, 12).padding(.vertical, 4)
                .glassSurface(Capsule())
                .frame(maxWidth: 220)
                .animation(.easeInOut(duration: 0.2), value: presenceText)
            }
        }.buttonStyle(.plain)
            .accessibilityElement(children: .ignore)
            .accessibilityLabel("\(friend.displayName), \(presenceText)").accessibilityHint("Shows contact details").accessibilityAddTraits(.isButton)
    }

    /// "Online", "Connecting…", "Last seen 5 min ago" or "Offline" under the name.
    private var presenceText: String {
        if online { return "Online" }
        if probing { return "Connecting…" }
        guard let seen = bridge.presenceSeen[friendID] else { return "Offline" }
        return Self.lastSeen(Date(timeIntervalSince1970: seen / 1000))
    }
    static func lastSeen(_ date: Date, now: Date = Date()) -> String {
        let minutes = Int(now.timeIntervalSince(date) / 60)
        if minutes < 1 { return "Last seen just now" }
        if minutes < 60 { return "Last seen \(minutes) min ago" }
        if minutes < 24 * 60 { return "Last seen \(minutes / 60) hr ago" }
        if Calendar.current.isDateInYesterday(date) { return "Last seen yesterday" }
        return "Last seen \(date.formatted(.dateTime.month(.abbreviated).day()))"
    }

    /// Calm inline note over the composer while the friend can't be reached.
    private var offlineNote: some View {
        Text(holdOn.map { "\(friend.displayName) is offline. Messages wait on \($0) and arrive when they’re back." }
             ?? "\(friend.displayName) is offline. Messages will send when you’re both online with DropBeam open.")
            .font(.footnote).foregroundStyle(.secondary).multilineTextAlignment(.center)
            .fixedSize(horizontal: false, vertical: true)
            .frame(maxWidth: .infinity).padding(.horizontal, 28).padding(.top, 6).padding(.bottom, 8)
            .accessibilityIdentifier("chat.offlineNote")
    }

    /// Check now; while they stay offline, check again every 30 s (never faster).
    private func presenceLoop() async {
        var first = true
        while !Task.isCancelled {
            // Always on open (a fresh-looking stamp may be stale); after that only
            // while they read as offline — online friends are refreshed by the engine.
            if first || !online { await probe() }
            first = false
            try? await Task.sleep(for: .seconds(30))
        }
    }
    /// One active check. The header gives up waiting after 8 s (Offline) while the
    /// dial itself may still land and flip it to Online.
    private func probe() async {
        lastProbe = Date()
        if bridge.presence[friendID] != true { probing = true }
        let check = Task { @MainActor in (try? await bridge.checkPresence(id: friendID)) ?? false }
        let deadline = Task { @MainActor in
            try? await Task.sleep(for: .seconds(8))
            guard !Task.isCancelled, probing else { return }
            probing = false; probedOffline = true
        }
        let reached = await check.value
        deadline.cancel()
        probedOffline = !reached
        probing = false
    }

    @ViewBuilder private func messageRow(_ message: ChatMessage, index: Int) -> some View {
        let previous = index > 0 ? messages[index - 1] : nil
        let next = index + 1 < messages.count ? messages[index + 1] : nil
        let newCluster = previous.map { !Calendar.current.isDate($0.date, inSameDayAs: message.date) || message.ts - $0.ts > Self.clusterGap } ?? true
        let nextCluster = next.map { !Calendar.current.isDate($0.date, inSameDayAs: message.date) || $0.ts - message.ts > Self.clusterGap } ?? true
        let newRun = newCluster || previous?.fromMe != message.fromMe || previous?.deleted == true || message.ts - (previous?.ts ?? 0) > Self.runGap
        let lastInRun = nextCluster || next?.fromMe != message.fromMe || next?.deleted == true || (next?.ts ?? 0) - message.ts > Self.runGap
        let lastMine = message.fromMe && message.id == messages.last(where: { $0.fromMe && $0.deleted != true })?.id
        VStack(spacing: 0) {
            if newCluster {
                ChatDates.header(message.date).font(.caption).foregroundStyle(.secondary)
                    .frame(maxWidth: .infinity).padding(.top, index == 0 ? 6 : 16).padding(.bottom, 6)
            }
            if message.deleted == true {
                Text(message.fromMe ? "You unsent a message." : "\(friend.displayName) unsent a message.")
                    .font(.caption).foregroundStyle(.secondary).frame(maxWidth: .infinity).padding(.vertical, 8)
            } else {
                HStack(alignment: .bottom, spacing: 0) {
                    if message.fromMe { Spacer(minLength: 0) }
                    if !message.fromMe {
                        Group {
                            if lastInRun { ContactAvatar(friend: friend, size: 28) }
                            else { Color.clear.frame(width: 28, height: 28) }
                        }.padding(.trailing, 2)
                    }
                    ChatBubble(message: message, lastInRun: lastInRun, query: query, lifted: tapback?.id == message.id,
                        onReply: { reply = message; editing = nil }, onEdit: { editing = message; reply = nil },
                        onLongPress: { frame in instant { tapback = TapbackTarget(message: message, lastInRun: lastInRun, frame: frame) } })
                        .frame(maxWidth: max(120, viewport.width * 0.78 - (message.fromMe ? 0 : 30)), alignment: message.fromMe ? .trailing : .leading)
                    if !message.fromMe { Spacer(minLength: 0) }
                }
                .padding(.top, newRun && !newCluster ? 10 : newCluster ? 0 : 2)
                .padding(.top, message.reactions?.isEmpty == false ? 24 : 0)
                let via = !message.fromMe && lastInRun && !(message.kind == "file" && message.path == nil) ? message.via : nil
                if message.edited == true || via != nil || (lastMine && delivery(message) != nil) {
                    VStack(alignment: message.fromMe ? .trailing : .leading, spacing: 1) {
                        if message.edited == true {
                            Text("Edited").font(.caption2.weight(.medium)).foregroundStyle(message.fromMe ? ChatPalette.sent : Color.secondary)
                        }
                        if let via {
                            Text("via \(via)").font(.caption2).foregroundStyle(.secondary)
                        }
                        if lastMine, let status = delivery(message) {
                            Group {
                                if message.status == "held" { Text("\(Image(systemName: "server.rack")) \(status)") } else { Text(status) }
                            }
                            .font(.caption2.weight(.medium)).foregroundStyle(.secondary)
                            .multilineTextAlignment(.trailing).fixedSize(horizontal: false, vertical: true)
                            .padding(.leading, 40)
                        }
                    }
                    .frame(maxWidth: .infinity, alignment: message.fromMe ? .trailing : .leading)
                    .padding(message.fromMe ? .trailing : .leading, message.fromMe ? MessageBubbleShape.tail + 4 : 30 + MessageBubbleShape.tail + 4)
                    .padding(.top, 3)
                }
            }
        }
    }
    private func delivery(_ message: ChatMessage) -> String? {
        if message.kind == "file", let transfer = bridge.transfers.first(where: { $0.chatOnly == true && $0.id == message.fileXferId }), transfer.state != "completed" { return nil }
        if message.fileXferFailed == true { return nil }
        switch message.status {
        case "read": return "Read"
        case "delivered", "sent": return "Delivered"
        case "held": return ServerCopy.held(server: message.heldOn, friend: firstName)
        case "failed" where ServerCopy.note(message.serverNote, friend: firstName, server: message.heldOn) != nil:
            return ServerCopy.note(message.serverNote, friend: firstName, server: message.heldOn)
        // The engine keeps every undelivered message queued and retries it until it
        // lands, so neither state is a failure; offline it is simply waiting.
        case "sending", "failed": return knownOffline ? "Waiting to send" : "Sending…"
        default: return nil
        }
    }
    /// Presents/dismisses the Tapback cover without the modal slide.
    private func instant(_ change: () -> Void) {
        var transaction = Transaction(); transaction.disablesAnimations = true
        withTransaction(transaction, change)
    }
    private func scrollDown(_ proxy: ScrollViewProxy, animated: Bool = true) {
        if animated { withAnimation(.easeOut(duration: 0.22)) { proxy.scrollTo("thread-bottom", anchor: .bottom) } }
        else { proxy.scrollTo("thread-bottom", anchor: .bottom) }
    }
    private func scrollToMatch(_ proxy: ScrollViewProxy) {
        guard matches.indices.contains(matchIndex) else { return }
        withAnimation { proxy.scrollTo(matches[matchIndex], anchor: .center) }
    }
    private func searchFooter(_ proxy: ScrollViewProxy) -> some View {
        HStack {
            Text(query.isEmpty ? "" : matches.isEmpty ? "No matches" : "\(matchIndex + 1) of \(matches.count)").font(.footnote).foregroundStyle(.secondary)
            Spacer()
            Button { matchIndex = max(0, matchIndex - 1); scrollToMatch(proxy) } label: { Image(systemName: "chevron.up").frame(width: 44, height: 44) }.accessibilityLabel("Previous search result").disabled(matchIndex == 0 || matches.isEmpty)
            Button { matchIndex = min(matches.count - 1, matchIndex + 1); scrollToMatch(proxy) } label: { Image(systemName: "chevron.down").frame(width: 44, height: 44) }.accessibilityLabel("Next search result").disabled(matches.isEmpty || matchIndex >= matches.count - 1)
        }.padding(.leading, 18).padding(.trailing, 6).glassSurface(Capsule()).padding(.horizontal, 12).padding(.bottom, 2)
    }
}

/// iOS 26 floats the composer in a scroll-edge bar so the thread blurs beneath
/// it; earlier systems inset the scroll content the classic way.
private struct ComposerBar<Bar: View>: ViewModifier {
    @ViewBuilder var bar: Bar
    func body(content: Content) -> some View {
        if #available(iOS 26, *) { content.safeAreaBar(edge: .bottom, spacing: 0) { bar } }
        else { content.safeAreaInset(edge: .bottom, spacing: 0) { bar.background(.bar) } }
    }
}

private struct ComposerTopKey: PreferenceKey {
    static let defaultValue: CGFloat = .infinity
    static func reduce(value: inout CGFloat, nextValue: () -> CGFloat) { value = min(value, nextValue()) }
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

/// The Messages typing indicator: a received bubble with three pulsing dots and a
/// two-circle "thought" tail.
struct TypingBubble: View {
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    var body: some View {
        TimelineView(.animation(minimumInterval: 1 / 30, paused: reduceMotion)) { timeline in
            let t = timeline.date.timeIntervalSinceReferenceDate
            HStack(spacing: 5) {
                ForEach(0..<3) { index in
                    let phase = reduceMotion ? 0.5 : (sin(t * 5.5 - Double(index) * 0.9) + 1) / 2
                    Circle().fill(Color(uiColor: .systemGray)).frame(width: 8, height: 8)
                        .opacity(0.35 + 0.65 * phase).scaleEffect(0.85 + 0.2 * phase)
                }
            }
            .padding(.horizontal, 14).frame(height: 36)
            .background(ChatPalette.received, in: Capsule())
            .background(alignment: .bottomLeading) {
                ZStack(alignment: .bottomLeading) {
                    Circle().fill(ChatPalette.received).frame(width: 12, height: 12).offset(x: -1, y: 1)
                    Circle().fill(ChatPalette.received).frame(width: 6, height: 6).offset(x: -6, y: 6)
                }
            }
            .padding(.leading, MessageBubbleShape.tail)
        }.accessibilityLabel("Typing")
    }
}

