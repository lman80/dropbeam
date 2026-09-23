import SwiftUI

struct FriendsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var search = ""
    @State private var adding = false
    @State private var folderScan = false
    @State private var showingCode = false
    @State private var removing: Friend?
    @State private var sendingTo: Friend?
    @Namespace private var avatars
    private var filtered: [Friend] {
        bridge.friends.filter { $0.groupedUnder == nil }.filter { search.isEmpty || $0.name.localizedCaseInsensitiveContains(search) || $0.displayName.localizedCaseInsensitiveContains(search) }
    }
    private func isMine(_ friend: Friend) -> Bool {
        if friend.ownDevice { return true }
        guard let account = bridge.myDevice?.accountPub, !account.isEmpty else { return false }
        return friend.accountPub == account
    }
    var body: some View {
        NavigationStack {
            List {
                if search.isEmpty {
                    Section {
                        NavigationLink { LocationsView() } label: { RowLabel(title: "Locations", symbol: "externaldrive.fill", color: .teal) }
                        NavigationLink { SharedFoldersView() } label: { RowLabel(title: "Shared Folders", symbol: "folder.fill.badge.person.crop", color: .blue) }
                        Button { showingCode = true; Haptics.tap() } label: { RowLabel(title: "My DropBeam Code", symbol: "qrcode", color: .beam) }
                            .buttonStyle(.plain)
                    } footer: { Text("Friends add you by scanning your code. Shared Folders stay in sync with friends; Locations are drives they let you browse.") }
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
                            VStack(spacing: 12) {
                                Image(systemName: "person.2.fill").font(.system(size: 36)).foregroundStyle(.tint).accessibilityHidden(true)
                                Text("Good things are better shared.").font(.headline)
                                Text("Add a friend by scanning their DropBeam code, or share yours.").font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
                                Button { adding = true; Haptics.tap() } label: { Label("Add Friend", systemImage: "person.badge.plus").padding(.horizontal, 8) }
                                    .beamButton(prominent: true).padding(.top, 4)
                            }.frame(maxWidth: .infinity).padding(.vertical, 16)
                        }
                        ForEach(others) { friend in row(friend) }
                    } header: { Text("Friends") }.headerProminence(.increased)
                }
            }
            .beamList()
            .overlay { if !search.isEmpty && filtered.isEmpty { ContentUnavailableView.search(text: search) } }
            .navigationTitle("Friends")
            .searchable(text: $search, prompt: "Find a friend or device")
            .refreshable { try? await bridge.action("refreshRecipients") }
            .animation(.smooth, value: filtered.map(\.id))
            .toolbar { ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("Add Friend", systemImage: "person.badge.plus") { adding = true }
                    Button("Show My Code", systemImage: "qrcode") { showingCode = true }
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
            .sheet(isPresented: $showingCode) { MyCodeSheet().environmentObject(bridge) }
            .confirmationDialog(removing.map { "Remove \($0.displayName)?" } ?? "", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible, presenting: removing) { friend in
                Button(friend.ownDevice ? "Remove from Account" : "Remove Friend", role: .destructive) {
                    bridge.perform { try await bridge.removeFriend(id: friend.id) }
                }
            } message: { friend in Text(friend.ownDevice ? "It stops syncing your friends and chats." : "You can add \(friend.name) again with their code.") }
            .confirmationDialog(sendingTo.map { "Send to \($0.displayName)" } ?? "", isPresented: Binding(get: { sendingTo != nil }, set: { if !$0 { sendingTo = nil } }), titleVisibility: .visible, presenting: sendingTo) { friend in
                Button("Photos") { bridge.perform { try await bridge.pickAndSend(source: "photos", friendId: friend.id) } }
                Button("Files") { bridge.perform { try await bridge.pickAndSend(source: "files", friendId: friend.id) } }
                Button("Folder") { bridge.perform { try await bridge.pickAndSend(source: "folder", friendId: friend.id) } }
            }
        }
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
        }
        .contextMenu {
            Button("Send Photos", systemImage: "photo.on.rectangle") { bridge.perform { try await bridge.pickAndSend(source: "photos", friendId: friend.id) } }
            Button("Send Files", systemImage: "doc") { bridge.perform { try await bridge.pickAndSend(source: "files", friendId: friend.id) } }
            Button("Send a Folder", systemImage: "folder") { bridge.perform { try await bridge.pickAndSend(source: "folder", friendId: friend.id) } }
            Button("Message", systemImage: "bubble.left") { bridge.perform { try await bridge.openChat(friendId: friend.id) } }
            Divider()
            Button(friend.ownDevice ? "Remove from Account" : "Remove Friend", systemImage: "person.fill.xmark", role: .destructive) { removing = friend }
        }
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

struct AddFriendSheet: View {
    @EnvironmentObject private var bridge: Bridge
    var body: some View {
        QRScannerSheet(title: "Add Friend", hint: "Scan the QR code on your friend’s Profile, or paste the code they sent you.") { code in
            if Bridge.isLinkCode(code) { let r = try await bridge.linkWithScannedCode(code); bridge.showToast("Linked with \(r.name ?? "your device")") }
            else if code.lowercased().hasPrefix("dropbeamf1:") { try await bridge.acceptFriend(code: code) }
            else { try await bridge.addFriendByCode(code: code) }
        }
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
                .navigationTitle("My Code").navigationBarTitleDisplayMode(.inline)
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
    private var friend: Friend { bridge.friends.first { $0.id == friendID } ?? initial }
    var body: some View {
        List {
            Section {
                VStack(spacing: 10) {
                    ContactAvatar(friend: friend, size: 104).padding(6).background(.ultraThinMaterial, in: Circle())
                        .overlay(Circle().strokeBorder(.white.opacity(0.35), lineWidth: 1))
                    Text(friend.displayName).font(.title.bold()).multilineTextAlignment(.center)
                    if friend.ownDevice && friend.name != friend.displayName { Text(friend.name).font(.subheadline).foregroundStyle(.secondary) }
                    PresenceLabel(online: bridge.presence[friendID] == true)
                    if let check { Text(check).font(.footnote).foregroundStyle(.secondary).accessibilityAddTraits(.updatesFrequently) }
                }.frame(maxWidth: .infinity).accessibilityElement(children: .combine)
            }.clearRow()
            Section {
                GlassGroup {
                    HStack(spacing: 10) {
                        action("Send", symbol: "paperplane.fill", prominent: true) { sendOptions = true; Haptics.tap() }
                        action("Message", symbol: "bubble.left.fill") { bridge.perform { try await bridge.openChat(friendId: friendID) } }
                        action(checking ? "Checking" : "Check", symbol: "wave.3.right") {
                            checking = true
                            bridge.perform { defer { checking = false }; check = try await bridge.pingFriend(id: friendID).label }
                        }.disabled(checking)
                    }
                }
            }.clearRow(EdgeInsets(top: 0, leading: 20, bottom: 8, trailing: 20))
            Section {
                IconToggle(title: "Accept Files Automatically", symbol: "tray.and.arrow.down.fill", color: .green, isOn: Binding(get: { friend.autoAccept ?? false }, set: { value in
                    bridge.perform { try await bridge.setAutoAccept(id: friendID, bool: value) }
                }))
                ActionRow(title: "Rename", symbol: "pencil", color: .orange) { name = friend.name; renaming = true }
            } footer: { Text("When on, files from \(friend.displayName) are saved without asking first.") }
            Section {
                NavigationLink { LocationsView(friendID: friendID) } label: { RowLabel(title: "Browse Locations", symbol: "externaldrive.fill", color: .teal) }
            }
            Section {
                Button(friend.ownDevice ? "Remove from Account" : "Remove Friend", role: .destructive) { removing = true; Haptics.warning() }
                    .frame(maxWidth: .infinity)
            }
        }
        .beamList()
        .navigationTitle(friend.displayName).navigationBarTitleDisplayMode(.inline)
        .toolbar { ToolbarItem(placement: .principal) { Text("").accessibilityHidden(true) } } // the header already shows the name
        .onChange(of: bridge.friends.map(\.id)) { _, ids in if !ids.contains(friendID) { dismiss() } }
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
    private func action(_ label: String, symbol: String, prominent: Bool = false, tap: @escaping () -> Void) -> some View {
        Button(action: tap) {
            VStack(spacing: 6) { Image(systemName: symbol).font(.title3); Text(label).font(.footnote.weight(.semibold)).lineLimit(1).minimumScaleFactor(0.8) }
                .frame(maxWidth: .infinity, minHeight: 54)
        }.beamButton(prominent: prominent)
    }
    private func pick(_ source: String) { bridge.perform { try await bridge.pickAndSend(source: source, friendId: friendID) } }
}
