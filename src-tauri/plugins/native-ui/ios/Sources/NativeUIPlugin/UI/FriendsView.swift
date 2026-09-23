import SwiftUI

struct FriendsView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var search = ""
    @State private var adding = false
    @State private var folderScan = false
    @Namespace private var avatars
    private var filtered: [Friend] {
        bridge.friends.filter { search.isEmpty || $0.name.localizedCaseInsensitiveContains(search) || $0.displayName.localizedCaseInsensitiveContains(search) }
    }
    private func isMine(_ friend: Friend) -> Bool {
        guard let account = bridge.myDevice?.accountPub, !account.isEmpty else { return false }
        return friend.ownDevice || friend.accountPub == account
    }
    var body: some View {
        NavigationStack {
            ScrollView {
                GlassGroup {
                    VStack(alignment: .leading, spacing: 24) {
                        NavigationLink { LocationsView() } label: {
                            GlassCard { SettingsLinkLabel(title: "Locations", symbol: "externaldrive") }
                        }.buttonStyle(.plain)
                        section("My Devices", friends: filtered.filter(isMine))
                        section("Friends", friends: filtered.filter { !isMine($0) })
                    }
                }.padding(20)
            }
            .contentMargins(.bottom, 24, for: .scrollContent)
            .navigationTitle("Friends").beamCanvas()
            .searchable(text: $search, prompt: "Find a friend or device")
            .toolbar { ToolbarItem(placement: .topBarTrailing) {
                Menu {
                    Button("Add Friend", systemImage: "person.badge.plus") { adding = true }
                    Button("Join Shared Folder", systemImage: "qrcode.viewfinder") { folderScan = true }
                } label: { Image(systemName: "plus").frame(width: 44, height: 44) }.accessibilityLabel("Add friend or folder")
            } }
            .sheet(isPresented: $folderScan) {
                QRScannerSheet(title: "Join Shared Folder") { code in
                    let accepted: Bool = try await bridge.call("acceptFolderInvite", ["code": code])
                    if !accepted { throw NSError(domain: "DropBeam", code: 1, userInfo: [NSLocalizedDescriptionKey: "Choose a folder to join, or cancel."]) }
                    bridge.showToast("Joined shared folder")
                }
            }
            .sheet(isPresented: $adding) { AddFriendSheet().environmentObject(bridge) }
        }
    }
    @ViewBuilder private func section(_ title: String, friends: [Friend]) -> some View {
        VStack(alignment: .leading, spacing: 12) {
            Text(title).font(.title2.weight(.semibold))
            if friends.isEmpty {
                GlassCard {
                    if title == "My Devices" && search.isEmpty {
                        NavigationLink { DevicesView() } label: { SettingsLinkLabel(title: "Link Your Other Devices", symbol: "laptopcomputer.and.iphone") }.buttonStyle(.plain)
                    } else {
                        Text(search.isEmpty ? "Good things are better shared. Add your first friend with +." : "No matches yet.")
                            .foregroundStyle(.secondary).font(.body)
                    }
                }
            }
            ForEach(friends) { friend in
                NavigationLink {
                    FriendDetailView(friendID: friend.id, initial: friend)
                        .friendTransition(id: friend.id, namespace: avatars)
                } label: {
                    GlassCard {
                        HStack(spacing: 14) {
                            ContactAvatar(friend: friend).matchedGeometryEffect(id: friend.id, in: avatars)
                            VStack(alignment: .leading, spacing: 5) {
                                Text(friend.displayName).font(.headline).foregroundStyle(.primary)
                                if friend.ownDevice { Text(friend.name).font(.footnote).foregroundStyle(.secondary).lineLimit(1) }
                                PresenceLabel(online: bridge.presence[friend.id] == true)
                            }
                            Spacer(minLength: 4)
                            if !friend.ownDevice, let glyph = deviceSymbol(friend.deviceKind) { Image(systemName: glyph).foregroundStyle(.secondary) }
                            Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary)
                        }
                    }
                    .contentShape(Rectangle())
                    .friendTransitionSource(id: friend.id, namespace: avatars)
                }.buttonStyle(.plain)
            }
        }.animation(.smooth, value: friends.map(\.id))
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
        QRScannerSheet(title: "Add Friend") { code in
            if Bridge.isLinkCode(code) { let r = try await bridge.linkWithScannedCode(code); bridge.showToast("Linked with \(r.name ?? "your device")") }
            else if code.lowercased().hasPrefix("dropbeamf1:") { try await bridge.acceptFriend(code: code) }
            else { try await bridge.addFriendByCode(code: code) }
        }
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
        ScrollView {
            VStack(spacing: 24) {
                VStack(spacing: 14) {
                    FriendAvatar(friend: friend, size: 96).padding(9).background(.ultraThinMaterial, in: Circle())
                        .overlay(Circle().strokeBorder(.white.opacity(0.35), lineWidth: 1))
                    Text(friend.name).font(.largeTitle.bold()).multilineTextAlignment(.center)
                    PresenceLabel(online: bridge.presence[friendID] == true)
                }.padding(.vertical, 12)
                GlassGroup {
                    ViewThatFits(in: .horizontal) {
                        HStack(alignment: .top, spacing: 10) { actions }
                        VStack(spacing: 12) { actions }
                    }
                }
                if let check { Text(check).font(.footnote).foregroundStyle(.secondary).accessibilityAddTraits(.updatesFrequently) }
                GlassCard {
                    VStack(spacing: 20) {
                        Toggle("Accept files automatically", isOn: Binding(get: { friend.autoAccept ?? false }, set: { value in
                            bridge.perform { try await bridge.setAutoAccept(id: friendID, bool: value) }
                        }))
                        Divider()
                        Button { name = friend.name; renaming = true; Haptics.tap() } label: {
                            HStack { Label("Rename", systemImage: "pencil"); Spacer(); Image(systemName: "chevron.right") }
                        }
                    }
                }
                NavigationLink { LocationsView(friendID: friendID) } label: { GlassCard { SettingsLinkLabel(title: "Browse Locations", symbol: "externaldrive") } }.buttonStyle(.plain)
                Button("Remove Friend", role: .destructive) { removing = true; Haptics.tap() }.beamButton()
            }.padding(20)
        }.navigationTitle("Friend").navigationBarTitleDisplayMode(.inline).beamCanvas()
            .onChange(of: bridge.friends.map(\.id)) { _, ids in if !ids.contains(friendID) { dismiss() } }
            .confirmationDialog("Send to \(friend.name)", isPresented: $sendOptions, titleVisibility: .visible) {
                Button("Photos") { pick("photos") }
                Button("Files") { pick("files") }
            }
            .alert("Rename Friend", isPresented: $renaming) {
                TextField("Name", text: $name)
                Button("Cancel", role: .cancel) {}
                Button("Save") { bridge.perform { try await bridge.renameFriend(id: friendID, name: name.trimmingCharacters(in: .whitespacesAndNewlines)) } }
                    .disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            .confirmationDialog("Remove \(friend.name)?", isPresented: $removing, titleVisibility: .visible) {
                Button("Remove Friend", role: .destructive) { bridge.perform { try await bridge.removeFriend(id: friendID); if !bridge.friends.contains(where: { $0.id == friendID }) { dismiss() } } }
            }
    }
    @ViewBuilder private var actions: some View {
        action("Send Files", symbol: "paperplane.fill") { sendOptions = true; Haptics.tap() }
        action("Message", symbol: "bubble.left.fill") { bridge.perform { try await bridge.openChat(friendId: friendID) } }
        action(checking ? "Checking…" : "Check", symbol: "wave.3.right") {
            checking = true
            bridge.perform { defer { checking = false }; check = try await bridge.pingFriend(id: friendID).label }
        }.disabled(checking)
    }
    private func action(_ label: String, symbol: String, tap: @escaping () -> Void) -> some View {
        Button(action: tap) {
            VStack(spacing: 8) { Image(systemName: symbol).font(.title2); Text(label).font(.footnote.weight(.semibold)) }
                .frame(maxWidth: .infinity).padding(.vertical, 10)
        }.beamButton()
    }
    private func pick(_ source: String) { bridge.perform { try await bridge.pickAndSend(source: source, friendId: friendID) } }
}
