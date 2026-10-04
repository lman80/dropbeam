import SwiftUI

struct FriendsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var search = ""
    @State private var adding = false
    @State private var folderScan = false
    @State private var showingCode = false
    @State private var removing: Friend?
    @State private var sendingTo: Friend?
    @State private var blocking: Friend?
    @State private var reporting: ReportTarget?
    @State private var blockingRequest: FriendRequest?
    @Namespace private var avatars
    private var filtered: [Friend] {
        bridge.friends.filter { $0.groupedUnder == nil }.filter { search.isEmpty || $0.name.localizedCaseInsensitiveContains(search) || $0.displayName.localizedCaseInsensitiveContains(search) }
    }
    /// "Mong and Mong might be the same person…" (first such group only; quiet).
    private func lookAlikeHint(_ people: [Friend]) -> String? {
        guard let f = people.first(where: { !$0.lookAlikeWith.isEmpty }) else { return nil }
        let names = [f.displayName] + f.lookAlikeWith
        let joined = names.count == 2 ? "\(names[0]) and \(names[1])" : names.dropLast().joined(separator: ", ") + " and " + (names.last ?? "")
        return "\(joined) might be the same person on two devices — ask them to link their devices (Settings → Devices)."
    }
    private func isMine(_ friend: Friend) -> Bool {
        if friend.ownDevice { return true }
        guard let account = bridge.myDevice?.accountPub, !account.isEmpty else { return false }
        return friend.accountPub == account
    }
    var body: some View {
        NavigationStack {
            List {
                // S2: people who introduced themselves aren't friends until accepted.
                if search.isEmpty && !bridge.friendRequests.isEmpty { requestsSection }
                if search.isEmpty {
                    Section {
                        NavigationLink { LocationsView() } label: { RowLabel(title: "Locations", symbol: "externaldrive.fill", color: .teal) }
                        NavigationLink { SharedFoldersView() } label: { RowLabel(title: "Shared Folders", symbol: "folder.fill.badge.person.crop", color: .blue) }
                        Button { showingCode = true; Haptics.tap() } label: { RowLabel(title: "Invite Friends", symbol: "qrcode", color: .beam) }
                            .buttonStyle(.plain)
                    }
                }
                let mine = filtered.filter(isMine)
                let others = filtered.filter { !isMine($0) }
                if !mine.isEmpty || search.isEmpty {
                    Section {
                        if mine.isEmpty {
                            NavigationLink { DevicesView() } label: { RowLabel(title: "Link Your Other Devices", symbol: "laptopcomputer.and.iphone", color: .gray) }
                        }
                        ForEach(mine) { friend in row(friend) }
                    } header: { Text("My Devices") }.headerProminence(.increased)
                }
                if !others.isEmpty || search.isEmpty {
                    Section {
                        if others.isEmpty {
                            VStack(spacing: 6) {
                                Text("No Friends Yet").font(.headline)
                                Text("Scan a friend’s DropBeam code, or show them yours.").font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
                                Button("Add Friend") { adding = true; Haptics.tap() }.font(.body.weight(.semibold)).buttonStyle(.borderless).padding(.top, 6)
                            }.frame(maxWidth: .infinity).padding(.vertical, 14)
                        }
                        ForEach(others) { friend in row(friend) }
                    } header: { Text("Friends") } footer: {
                        if let hint = lookAlikeHint(others) { Text(hint) }
                    }.headerProminence(.increased)
                }
            }
            .beamList()
            .overlay { if !search.isEmpty && filtered.isEmpty { ContentUnavailableView.search(text: search) } }
            .navigationTitle("Friends")
            .searchable(text: $search, prompt: "Find a friend or device")
            .refreshable { try? await bridge.action("refreshRecipients"); await bridge.refreshFriendRequests() }
            .task { await bridge.refreshFriendRequests() }
            .animation(.smooth, value: filtered.map(\.id))
            .toolbar { ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("Add Friend", systemImage: "person.badge.plus") { adding = true }
                    Button("Share My Invite", systemImage: "qrcode") { showingCode = true }
                    Button("Join Shared Folder", systemImage: "folder.badge.plus") { folderScan = true }
                } label: { Image(systemName: "plus") }.accessibilityLabel("Add friend or folder")
            } }
            .sheet(isPresented: $folderScan) {
                QRScannerSheet(title: "Join Shared Folder") { code in
                    let accepted: Bool = try await bridge.call("acceptFolderInvite", ["code": code])
                    if !accepted { throw NSError(domain: "DropBeam", code: 1, userInfo: [NSLocalizedDescriptionKey: "Choose a folder to join, or cancel."]) }
                    bridge.showToast("Joined shared folder")
                }
            }
            .sheet(isPresented: $adding) { AddFriendSheet().environmentObject(bridge) }
            #if targetEnvironment(simulator)
            // QA: `-openAddFriend` / `-openInvite` (with `-openTab friends`).
            .onAppear {
                if CommandLine.arguments.contains("-openAddFriend") { adding = true }
                if CommandLine.arguments.contains("-openInvite") { showingCode = true }
            }
            #endif
            .sheet(isPresented: $showingCode) { MyCodeSheet().environmentObject(bridge) }
            .confirmationDialog(removing.map { "Remove \($0.displayName)?" } ?? "", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible, presenting: removing) { friend in
                Button(friend.ownDevice ? "Remove from Account" : "Remove Friend", role: .destructive) {
                    bridge.perform { try await bridge.removeFriend(id: friend.id) }
                }
            } message: { friend in Text(friend.ownDevice ? "It stops syncing your friends and chats." : "You can add \(friend.name) again with their code.") }
            .safetyPrompts(block: $blocking, report: $reporting)
            .modifier(BlockRequestPrompt(request: $blockingRequest))
            .confirmationDialog(sendingTo.map { "Send to \($0.displayName)" } ?? "", isPresented: Binding(get: { sendingTo != nil }, set: { if !$0 { sendingTo = nil } }), titleVisibility: .visible, presenting: sendingTo) { friend in
                Button("Photos") { bridge.perform { try await bridge.pickAndSend(source: "photos", friendId: friend.id) } }
                Button("Files") { bridge.perform { try await bridge.pickAndSend(source: "files", friendId: friend.id) } }
                Button("Folder") { bridge.perform { try await bridge.pickAndSend(source: "folder", friendId: friend.id) } }
            }
        }
    }
    private var requestsSection: some View {
        Section {
            ForEach(bridge.friendRequests) { request in requestRow(request) }
        } header: { Text("Friend Requests") } footer: {
            Text("They can’t message you, and their files always ask first, until you accept.")
        }.headerProminence(.increased)
    }
    private func requestRow(_ request: FriendRequest) -> some View {
        HStack(spacing: 14) {
            ContactAvatar(friend: Friend(id: request.endpointId, name: request.name), size: 46)
            VStack(alignment: .leading, spacing: 3) {
                Text(request.name).font(.body.weight(.semibold)).foregroundStyle(.primary).lineLimit(2)
                    .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                Text(requestSubtitle(request)).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
            }
            Spacer(minLength: 4)
            Button("Accept") {
                bridge.perform { try await bridge.acceptFriendRequest(endpointId: request.endpointId); Haptics.success(); bridge.showToast("\(request.name) is now your friend") }
            }.beamButton(prominent: true).controlSize(.small)
            Menu {
                Button("Decline", systemImage: "xmark") { bridge.perform { try await bridge.declineFriendRequest(endpointId: request.endpointId, block: false) } }
                Button("Decline and Block…", systemImage: "hand.raised", role: .destructive) { blockingRequest = request }
            } label: { Image(systemName: "ellipsis.circle").font(.title3).frame(width: 44, height: 44).contentShape(Rectangle()) }
                .buttonStyle(.borderless).accessibilityLabel("Decline \(request.name)")
        }
        .padding(.vertical, 2)
        .swipeActions(edge: .trailing) {
            Button(role: .destructive) { bridge.perform { try await bridge.declineFriendRequest(endpointId: request.endpointId, block: false) } } label: { Label("Decline", systemImage: "xmark") }
            Button { blockingRequest = request } label: { Label("Block", systemImage: "hand.raised.fill") }.tint(.orange)
        }
        .contextMenu {
            Button("Accept", systemImage: "checkmark") { bridge.perform { try await bridge.acceptFriendRequest(endpointId: request.endpointId) } }
            Button("Decline", systemImage: "xmark") { bridge.perform { try await bridge.declineFriendRequest(endpointId: request.endpointId, block: false) } }
            Button("Decline and Block…", systemImage: "hand.raised", role: .destructive) { blockingRequest = request }
        }
    }
    private func requestSubtitle(_ request: FriendRequest) -> String {
        guard let ms = request.at else { return "Wants to be your friend" }
        let date = Date(timeIntervalSince1970: ms / 1000)
        return "Wants to be your friend · " + (Date().timeIntervalSince(date) < 60 ? "just now" : date.formatted(.relative(presentation: .named)))
    }
    private func row(_ friend: Friend) -> some View {
        NavigationLink {
            FriendDetailView(friendID: friend.id, initial: friend)
                .friendTransition(id: friend.id, namespace: avatars)
        } label: {
            HStack(spacing: 14) {
                ContactAvatar(friend: friend, size: 46).friendTransitionSource(id: friend.id, namespace: avatars)
                VStack(alignment: .leading, spacing: 3) {
                    Text(friend.displayName).font(.body.weight(.semibold)).foregroundStyle(.primary).lineLimit(2)
                        .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                    if friend.ownDevice && friend.name != friend.displayName { Text(friend.name).font(.subheadline).foregroundStyle(.secondary).lineLimit(1) }
                    PresenceLabel(online: bridge.presence[friend.id] == true)
                }
                Spacer(minLength: 4)
                if !friend.ownDevice, let glyph = deviceSymbol(friend.deviceKind) { Image(systemName: glyph).foregroundStyle(.secondary).accessibilityHidden(true) }
            }.padding(.vertical, 2)
        }
        .accessibilityElement(children: .combine)
        .swipeActions(edge: .leading, allowsFullSwipe: true) {
            Button { sendingTo = friend } label: { Label("Send", systemImage: "paperplane.fill") }.tint(.beam)
            Button { bridge.perform { try await bridge.openChat(friendId: friend.id) } } label: { Label("Message", systemImage: "bubble.left.fill") }.tint(.blue)
        }
        .swipeActions(edge: .trailing) {
            Button(role: .destructive) { removing = friend } label: { Label("Remove", systemImage: "person.fill.xmark") }
            if !isMine(friend) { Button { blocking = friend } label: { Label("Block", systemImage: "hand.raised.fill") }.tint(.orange) }
        }
        .contextMenu {
            Button("Send Photos", systemImage: "photo.on.rectangle") { bridge.perform { try await bridge.pickAndSend(source: "photos", friendId: friend.id) } }
            Button("Send Files", systemImage: "doc") { bridge.perform { try await bridge.pickAndSend(source: "files", friendId: friend.id) } }
            Button("Send a Folder", systemImage: "folder") { bridge.perform { try await bridge.pickAndSend(source: "folder", friendId: friend.id) } }
            Button("Message", systemImage: "bubble.left") { bridge.perform { try await bridge.openChat(friendId: friend.id) } }
            Divider()
            if !isMine(friend) {
                Button("Report…", systemImage: "exclamationmark.bubble") { reporting = ReportTarget(friend: friend) }
                Button("Block…", systemImage: "hand.raised", role: .destructive) { blocking = friend }
            }
            Button(friend.ownDevice ? "Remove from Account" : "Remove Friend", systemImage: "person.fill.xmark", role: .destructive) { removing = friend }
        }
    }
}

/// "Block Jordan?" for a friend request: declines it and blocks them everywhere.
private struct BlockRequestPrompt: ViewModifier {
    @EnvironmentObject private var bridge: Bridge
    @Binding var request: FriendRequest?
    func body(content: Content) -> some View {
        content.confirmationDialog(request.map { "Block \($0.name)?" } ?? "", isPresented: Binding(get: { request != nil }, set: { if !$0 { request = nil } }), titleVisibility: .visible, presenting: request) { request in
            Button("Decline and Block", role: .destructive) {
                bridge.perform { try await bridge.declineFriendRequest(endpointId: request.endpointId, block: true); bridge.showToast("\(request.name) was blocked") }
            }
        } message: { _ in Text("Their requests, messages and files are stopped on all your devices. They aren’t told.") }
    }
}

private extension View {
    @ViewBuilder func friendTransition(id: String, namespace: Namespace.ID) -> some View {
        if #available(iOS 18, *) { navigationTransition(.zoom(sourceID: id, in: namespace)) }
        else { self }
    }
    @ViewBuilder func friendTransitionSource(id: String, namespace: Namespace.ID) -> some View {
        if #available(iOS 18, *) { matchedTransitionSource(id: id, in: namespace) }
        else { self }
    }
}

/// Your own code as a sheet (from Friends → +), so a friend can scan it right away.
struct MyCodeSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    var body: some View {
        NavigationStack {
            List { MyCodeSection() }
                .beamList()
                .navigationTitle("Invite Friends").navigationBarTitleDisplayMode(.inline)
                .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }.presentationDetents([.large]).tint(.beam)
    }
}

struct FriendDetailView: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let friendID: String
    let initial: Friend
    @State private var sendOptions = false
    @State private var renaming = false
    @State private var removing = false
    @State private var name = ""
    @State private var check: String?
    @State private var checking = false
    @State private var blocking: Friend?
    @State private var reporting: ReportTarget?
    @State private var headerHidden = false
    private var friend: Friend { bridge.friends.first { $0.id == friendID } ?? initial }
    private var isMine: Bool {
        if friend.ownDevice { return true }
        guard let account = bridge.myDevice?.accountPub, !account.isEmpty else { return false }
        return friend.accountPub == account
    }
    var body: some View {
        List {
            Section {
                VStack(spacing: 8) {
                    ContactAvatar(friend: friend, size: 96)
                    Text(friend.displayName).font(.title2.bold()).multilineTextAlignment(.center).lineLimit(3)
                    if friend.ownDevice && friend.name != friend.displayName { Text(friend.name).font(.subheadline).foregroundStyle(.secondary) }
                    PresenceLabel(online: bridge.presence[friendID] == true)
                }.frame(maxWidth: .infinity).accessibilityElement(children: .combine)
            }.clearRow(EdgeInsets(top: 0, leading: 20, bottom: 4, trailing: 20))
            Section {
                ActionTileRow {
                    ActionTile(title: "Send", symbol: "paperplane") { sendOptions = true; Haptics.tap() }
                    ActionTile(title: "Message", symbol: "message") { bridge.perform { try await bridge.openChat(friendId: friendID) } }
                    ActionTile(title: "Locations", symbol: "externaldrive") { browsing = true; Haptics.tap() }
                }
            }.clearRow(EdgeInsets(top: 0, leading: 20, bottom: 8, trailing: 20))
            Section {
                IconToggle(title: "Accept Files Automatically", symbol: "tray.and.arrow.down.fill", color: .green, isOn: Binding(get: { friend.autoAccept ?? false }, set: { value in
                    bridge.perform { try await bridge.setAutoAccept(id: friendID, bool: value) }
                }))
            } footer: { Text("Files from \(friend.displayName) are saved without asking first.") }
            if !isMine, let other = friend.lookAlikeWith.first {
                Section {} footer: {
                    Text("This might be the same person as your contact “\(other)” on another device. Ask them to link their devices (Settings → Devices).")
                }
            }
            Section {
                Button {
                    checking = true; check = nil
                    bridge.perform { defer { checking = false }; check = try await bridge.pingFriend(id: friendID).plainLabel }
                } label: {
                    HStack {
                        Text("Test Connection").foregroundStyle(.tint)
                        Spacer(minLength: 8)
                        if checking { ProgressView() }
                        else if let check { Text(check).foregroundStyle(.secondary).multilineTextAlignment(.trailing) }
                    }.contentShape(Rectangle())
                }.buttonStyle(.plain).disabled(checking)
                    .accessibilityElement(children: .combine).accessibilityAddTraits(.updatesFrequently)
            }
            Section {
                if !isMine {
                    Button("Report \(friend.displayName)…") { reporting = ReportTarget(friend: friend) }
                    Button("Block \(friend.displayName)", role: .destructive) { blocking = friend; Haptics.warning() }
                }
                Button(friend.ownDevice ? "Remove from Account" : "Remove Friend", role: .destructive) { removing = true; Haptics.warning() }
            } footer: {
                if !isMine { Text("Blocking stops their messages, files and invites on all your devices. They aren’t told.") }
            }
        }
        .beamList()
        .onScrollHeaderHidden($headerHidden, threshold: 150)
        .navigationTitle(headerHidden ? friend.displayName : "").navigationBarTitleDisplayMode(.inline)
        .navigationDestination(isPresented: $browsing) { LocationsView(friendID: friendID) }
        .toolbar { ToolbarItem(placement: .topBarTrailing) { Button("Edit") { name = friend.name; renaming = true } } }
        .onChange(of: bridge.friends.map(\.id)) { _, ids in if !ids.contains(friendID) { dismiss() } }
        .safetyPrompts(block: $blocking, report: $reporting)
        .confirmationDialog("Send to \(friend.displayName)", isPresented: $sendOptions, titleVisibility: .visible) {
            Button("Photos") { pick("photos") }
            Button("Files") { pick("files") }
            Button("Folder") { pick("folder") }
        }
        .alert("Rename", isPresented: $renaming) {
            TextField("Name", text: $name)
            Button("Cancel", role: .cancel) {}
            Button("Save") { bridge.perform { try await bridge.renameFriend(id: friendID, name: name.trimmingCharacters(in: .whitespacesAndNewlines)) } }
                .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
        } message: { Text("Only you see this name.") }
        .confirmationDialog("Remove \(friend.displayName)?", isPresented: $removing, titleVisibility: .visible) {
            Button(friend.ownDevice ? "Remove from Account" : "Remove Friend", role: .destructive) { bridge.perform { try await bridge.removeFriend(id: friendID); if !bridge.friends.contains(where: { $0.id == friendID }) { dismiss() } } }
        } message: { Text(friend.ownDevice ? "It stops syncing your friends and chats." : "You can add \(friend.name) again with their code.") }
    }
    @State private var browsing = false
    private func pick(_ source: String) { bridge.perform { try await bridge.pickAndSend(source: source, friendId: friendID) } }
}

extension View {
    /// Flips `hidden` once the list scrolls past `threshold` points — used to fade a
    /// big page header's name into the navigation bar (iOS 18+; earlier keeps the header).
    @ViewBuilder func onScrollHeaderHidden(_ hidden: Binding<Bool>, threshold: CGFloat) -> some View {
        if #available(iOS 18, *) {
            onScrollGeometryChange(for: Bool.self) { geo in geo.contentOffset.y + geo.contentInsets.top > threshold } action: { _, value in
                withAnimation(.easeOut(duration: 0.15)) { hidden.wrappedValue = value }
            }
        } else { self }
    }
}
