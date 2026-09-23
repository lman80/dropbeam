import SwiftUI
import UIKit

/// Friends → Shared Folders: every folder this iPhone syncs with friends (joined
/// from an invite). Mirrors desktop FoldersView: live status, members and roles,
/// pause/resume, verify, recoverable files, invite, leave. Creating a folder stays
/// on a computer — an iPhone can only sync while DropBeam is open.
struct SharedFoldersView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var joining = false
    @State private var loading = false
    var body: some View {
        ScrollView {
            GlassGroup {
                VStack(alignment: .leading, spacing: 24) {
                    if bridge.folders.isEmpty {
                        if loading { ProgressView("Loading shared folders…").frame(maxWidth: .infinity).padding(.vertical, 32) }
                        else {
                            ContentUnavailableView {
                                Label("No shared folders yet", systemImage: "folder.badge.person.crop")
                            } description: {
                                Text("When a friend shares a folder with you from DropBeam on their computer, accept the invite here — or scan their invite code.")
                            } actions: {
                                Button { joining = true; Haptics.tap() } label: { Label("Join with a Code", systemImage: "qrcode.viewfinder") }.beamButton(prominent: true)
                            }.padding(.top, 24)
                        }
                    }
                    ForEach(bridge.folders) { folder in
                        NavigationLink { SharedFolderDetailView(folderID: folder.id, initial: folder) } label: { FolderCard(folder: folder) }.buttonStyle(.plain)
                    }
                    FolderIOSNote()
                }.padding(20)
            }
        }
        .contentMargins(.bottom, 24, for: .scrollContent)
        .navigationTitle("Shared Folders").navigationBarTitleDisplayMode(.large).beamCanvas()
        .toolbar { ToolbarItem(placement: .topBarTrailing) {
            Button { joining = true; Haptics.tap() } label: { Image(systemName: "plus").frame(width: 44, height: 44) }.accessibilityLabel("Join a shared folder")
        } }
        .task { loading = bridge.folders.isEmpty; defer { loading = false }; await refresh() }
        .refreshable { await refresh() }
        .sheet(isPresented: $joining) { JoinFolderSheet() }
    }
    private func refresh() async { let _: IgnoredResult? = try? await bridge.call("foldersRefresh") }
}

/// Scan or paste a shared-folder invite, then pick where its copy lives.
struct JoinFolderSheet: View {
    @EnvironmentObject private var bridge: Bridge
    var body: some View {
        QRScannerSheet(title: "Join Shared Folder") { code in
            let accepted: Bool = try await bridge.call("acceptFolderInvite", ["code": code])
            if !accepted { throw NSError(domain: "DropBeam", code: 1, userInfo: [NSLocalizedDescriptionKey: "Choose a folder to join, or cancel."]) }
            bridge.showToast("Joined shared folder")
        }
    }
}

/// What iOS allows, said once, plainly.
private struct FolderIOSNote: View {
    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            Label("On iPhone", systemImage: "iphone").font(.footnote.weight(.semibold)).foregroundStyle(.secondary)
            Text("Each shared folder is a copy in Files → On My iPhone → DropBeam. Syncing runs while DropBeam is open; iOS pauses it in the background, and changes catch up the next time you open the app. Create new shared folders in DropBeam on a computer.")
                .font(.footnote).foregroundStyle(.secondary)
        }.padding(.horizontal, 4)
    }
}

private func toneColor(_ tone: String) -> Color {
    switch tone {
    case "busy": return .beam
    case "warn": return .orange
    case "error": return .red
    case "offline": return .secondary
    default: return .green
    }
}
private func statusText(_ folder: SharedFolder) -> String {
    guard folder.tone == "ok", let ms = folder.lastSyncedMs else { return folder.label }
    return "\(folder.label) · synced \(Date(timeIntervalSince1970: ms / 1000).formatted(.relative(presentation: .named)))"
}
private func peopleLine(_ folder: SharedFolder) -> String {
    let names = folder.members.filter { !$0.pending }.map(\.name)
    if names.isEmpty { return folder.members.isEmpty ? folder.modeLabel : "Waiting for someone to join" }
    return "With " + ListFormatter.localizedString(byJoining: names)
}

private struct FolderStatusRow: View {
    let folder: SharedFolder
    var body: some View {
        HStack(spacing: 8) {
            Circle().fill(toneColor(folder.tone)).frame(width: 8, height: 8)
            Text(statusText(folder)).font(.subheadline).foregroundStyle(.secondary).lineLimit(3)
        }.accessibilityElement(children: .combine).accessibilityAddTraits(.updatesFrequently)
    }
}

private struct FolderCard: View {
    let folder: SharedFolder
    var body: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 12) {
                HStack(spacing: 14) {
                    FileGlyph(name: "", symbol: "folder.fill")
                    VStack(alignment: .leading, spacing: 4) {
                        Text(folder.name).font(.headline).foregroundStyle(.primary).lineLimit(2)
                        Text(peopleLine(folder)).font(.subheadline).foregroundStyle(.secondary).lineLimit(1)
                    }
                    Spacer(minLength: 0)
                    if folder.paused { Image(systemName: "pause.circle.fill").foregroundStyle(.orange).accessibilityLabel("Paused") }
                    Image(systemName: "chevron.right").font(.footnote.weight(.semibold)).foregroundStyle(.tertiary)
                }
                FolderStatusRow(folder: folder)
                if folder.busy { ProgressView(value: folder.percent, total: 100).tint(.beam) }
            }
        }.contentShape(Rectangle())
    }
}

struct SharedFolderDetailView: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let folderID: String
    let initial: SharedFolder
    @State private var leaving = false
    @State private var removing: FolderMember?
    @State private var inviting = false
    @State private var code: InviteCode?
    @State private var busy: String?
    @State private var verify: FolderVerify?
    private var folder: SharedFolder { bridge.folders.first { $0.id == folderID } ?? initial }
    private var megabits: Bool { bridge.settings?.showMegabits == true }
    var body: some View {
        ScrollView {
            GlassGroup {
                VStack(alignment: .leading, spacing: 24) {
                    header
                    statusCard
                    actions
                    if let verify { verifyCard(verify) }
                    members
                    if folder.mirror {
                        NavigationLink { FolderRecoverableView(folderID: folder.id, pairID: folder.pairId, name: folder.name) } label: {
                            GlassCard { SettingsLinkLabel(title: "Recoverable Files", symbol: "clock.arrow.circlepath") }
                        }.buttonStyle(.plain)
                    }
                    Button(role: .destructive) { leaving = true; Haptics.tap() } label: { Label("Leave Folder", systemImage: "rectangle.portrait.and.arrow.right").frame(maxWidth: .infinity, minHeight: 36) }.beamButton().tint(.red)
                        .confirmationDialog("Leave \(folder.name)?", isPresented: $leaving, titleVisibility: .visible) {
                            Button("Leave Folder", role: .destructive) { run("leave") { try await bridge.action("folderLeave", ["folderId": folderID]); bridge.showToast("Left \(folder.name)"); dismiss() } }
                        } message: { Text("It stops syncing with everyone. Files already on this iPhone stay in Files.") }
                    Text("Leaving stops syncing on this iPhone and tells the others. Your copy stays in Files.").font(.footnote).foregroundStyle(.secondary)
                }.padding(20)
            }
        }
        .contentMargins(.bottom, 24, for: .scrollContent)
        .navigationTitle(folder.name).navigationBarTitleDisplayMode(.inline).beamCanvas()
        .onChange(of: bridge.folders.map(\.id)) { _, ids in if !ids.contains(folderID) { dismiss() } }
        .confirmationDialog(removing.map { $0.pending ? "Cancel this invite?" : "Remove \($0.name)?" } ?? "", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible) {
            if let member = removing {
                Button(member.pending ? "Cancel Invite" : "Remove from Folder", role: .destructive) { run("member") { try await bridge.action("folderRemoveMember", ["folderId": folderID, "pairId": member.pairId]) } }
            }
        } message: { Text(removing?.pending == true ? "Anyone you already sent this invite to won’t be able to join with it." : "They’ll stop syncing this folder with you.") }
        .sheet(isPresented: $inviting) { InviteFriendsSheet(folder: folder) }
        .sheet(item: $code) { InviteCodeSheet(invite: $0) }
    }
    private var header: some View {
        VStack(spacing: 12) {
            Image(systemName: "folder.fill").font(.system(size: 56)).foregroundStyle(.blue).padding(18)
                .background(Color.blue.opacity(0.12), in: RoundedRectangle(cornerRadius: 26)).accessibilityHidden(true)
            Text(folder.name).font(.title.bold()).multilineTextAlignment(.center)
            HStack(spacing: 8) {
                chip(folder.modeLabel, symbol: folder.mirror ? "arrow.triangle.2.circlepath" : folder.mode == "twoWay" ? "arrow.left.arrow.right" : "arrow.right", tint: .beam)
                if folder.paused { chip("Paused", symbol: "pause.fill", tint: .orange) }
                if folder.autoDelete { chip("Auto-delete", symbol: "trash", tint: .orange) }
            }
        }.frame(maxWidth: .infinity).padding(.vertical, 8)
    }
    private func chip(_ text: String, symbol: String, tint: Color) -> some View {
        Label(text, systemImage: symbol).font(.caption.weight(.semibold)).foregroundStyle(tint)
            .padding(.horizontal, 10).padding(.vertical, 5).background(tint.opacity(0.12), in: Capsule())
    }
    private var statusCard: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 12) {
                FolderStatusRow(folder: folder)
                if folder.peerUnshared {
                    Label("The others no longer share this folder. Your files are still here — you can leave it now.", systemImage: "exclamationmark.triangle.fill").font(.footnote).foregroundStyle(.red)
                }
                if folder.iAmViewer {
                    Label("View only: changes you make here are not sent.", systemImage: "eye").font(.footnote).foregroundStyle(.orange)
                }
                if folder.busy {
                    HStack(alignment: .firstTextBaseline) {
                        Text("\(Int(folder.percent))%").font(.title3.weight(.bold)).foregroundStyle(.tint)
                        if let locality = folder.locality, locality != "unknown" { Text(locality == "internet" ? "Relay" : locality.capitalized).font(.caption.weight(.semibold)).foregroundStyle(.secondary) }
                        Spacer()
                        if folder.state == "sending" {
                            Button { run("stop") { try await bridge.action("folderStop", ["folderId": folderID]) } } label: { Label("Stop", systemImage: "xmark") }.font(.footnote).beamButton()
                        }
                    }
                    ProgressView(value: folder.percent, total: 100).tint(.beam)
                    Text([folder.bytesTotal > 0 ? "\(Formatters.bytes(folder.bytesDone)) of \(Formatters.bytes(folder.bytesTotal))" : nil, Formatters.speed(folder.speedBps, megabits: megabits), Formatters.eta(folder.etaSeconds)].compactMap { $0 }.joined(separator: " · "))
                        .font(.footnote).foregroundStyle(.secondary)
                    if let file = folder.currentFile { Label(file, systemImage: Formatters.symbol(file)).font(.footnote).lineLimit(1) }
                    ForEach(Array(folder.queuedFiles.prefix(5).enumerated()), id: \.offset) { _, name in
                        Label(name, systemImage: "clock").font(.footnote).foregroundStyle(.secondary).lineLimit(1)
                    }
                    if folder.queuedFiles.count > 5 { Text("and \(folder.queuedFiles.count - 5) more queued").font(.footnote).foregroundStyle(.secondary) }
                }
                if folder.inSync {
                    Label(folder.peerFiles.map { "In sync — both sides have \($0) file\($0 == 1 ? "" : "s")" } ?? "In sync", systemImage: "checkmark.circle.fill").font(.footnote).foregroundStyle(.green)
                }
                if let s = folder.summary {
                    Text("\(s.direction == "send" ? "Sent" : "Received") \(s.files) file\(s.files == 1 ? "" : "s") · \(Formatters.bytes(s.bytes)) · \(Formatters.speed(s.avgBps, megabits: megabits)) avg")
                        .font(.footnote).foregroundStyle(.secondary)
                }
            }
        }
    }
    @ViewBuilder private var actions: some View {
        ViewThatFits(in: .horizontal) {
            HStack(alignment: .top, spacing: 10) { actionButtons }
            VStack(spacing: 12) { actionButtons }
        }
    }
    @ViewBuilder private var actionButtons: some View {
        if folder.mirror {
            action(folder.paused ? "Resume" : "Pause", symbol: folder.paused ? "play.fill" : "pause.fill") {
                run("pause") { try await bridge.action("folderSetPaused", ["folderId": folderID, "bool": !folder.paused]) }
            }
        }
        action("Open in Files", symbol: "folder") { openInFiles() }
        if folder.mirror {
            action(busy == "verify" ? "Checking…" : "Verify", symbol: "checkmark.shield") {
                verify = nil
                run("verify") { verify = try await bridge.call("folderVerify", ["folderId": folderID]) }
            }
        }
    }
    private func action(_ label: String, symbol: String, tap: @escaping () -> Void) -> some View {
        Button(action: tap) {
            VStack(spacing: 8) { Image(systemName: symbol).font(.title2); Text(label).font(.footnote.weight(.semibold)).lineLimit(1) }
                .frame(maxWidth: .infinity).padding(.vertical, 10)
        }.beamButton().disabled(busy != nil)
    }
    private func verifyCard(_ r: FolderVerify) -> some View {
        GlassCard {
            if !r.peerOnline || !r.compared {
                Label("Couldn’t compare right now — the others need to be online. Try again when they are.", systemImage: "wifi.exclamationmark").font(.subheadline)
            } else if r.identical {
                Label("Identical — \(r.matched) file\(r.matched == 1 ? "" : "s") match on both sides.", systemImage: "checkmark.seal.fill").font(.subheadline).foregroundStyle(.green)
            } else {
                Label("\(r.differences) difference\(r.differences == 1 ? "" : "s") found — syncing now to fix \(r.differences == 1 ? "it" : "them").", systemImage: "arrow.triangle.2.circlepath").font(.subheadline).foregroundStyle(.orange)
            }
        }
    }
    private var members: some View {
        VStack(alignment: .leading, spacing: 12) {
            Text("People").font(.title2.weight(.semibold))
            GlassCard {
                VStack(spacing: 0) {
                    HStack(spacing: 12) {
                        MyAvatar(size: 40)
                        VStack(alignment: .leading, spacing: 3) {
                            Text("\(bridge.settings?.displayName ?? "You") (you)").font(.headline)
                            Text(folder.iAmViewer ? "Viewer" : folder.iAmOwner ? "Owner" : "Editor").font(.footnote).foregroundStyle(.secondary)
                        }
                        Spacer(minLength: 0)
                    }.frame(minHeight: 52)
                    ForEach(folder.members) { member in
                        Divider().padding(.leading, 52)
                        memberRow(member)
                    }
                }
            }
            if !folder.members.contains(where: \.canSetRole) && folder.members.contains(where: { !$0.pending }) {
                Text("Only the person who created this folder can change who can edit.").font(.footnote).foregroundStyle(.secondary).padding(.horizontal, 4)
            }
            if !folder.iAmViewer {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 10) { inviteButtons }
                    VStack(spacing: 10) { inviteButtons }
                }
            }
        }
    }
    @ViewBuilder private var inviteButtons: some View {
        Button { inviting = true; Haptics.tap() } label: { Label("Invite a Friend", systemImage: "person.badge.plus").frame(maxWidth: .infinity, minHeight: 36) }.beamButton(prominent: true)
        Button {
            run("code") {
                let value: String = try await bridge.call("folderAddPerson", ["folderId": folderID])
                code = InviteCode(code: value, folderName: folder.name)
            }
        } label: { Label(busy == "code" ? "Making…" : "Share a Code", systemImage: "qrcode").frame(maxWidth: .infinity, minHeight: 36) }.beamButton().disabled(busy != nil)
    }
    private func memberRow(_ member: FolderMember) -> some View {
        let friend = member.friendId.flatMap { id in bridge.friends.first { $0.id == id } }
        return HStack(spacing: 12) {
            if let friend { ContactAvatar(friend: friend, size: 40) }
            else {
                Image(systemName: member.pending ? "hourglass" : "person.fill").foregroundStyle(.white).frame(width: 40, height: 40)
                    .background(member.pending ? Color.secondary : Color.beam, in: Circle()).accessibilityHidden(true)
            }
            VStack(alignment: .leading, spacing: 3) {
                Text(friend?.displayName ?? member.name).font(.headline).foregroundStyle(member.pending ? .secondary : .primary).lineLimit(1)
                if member.pending { Text("Hasn’t accepted yet").font(.footnote).foregroundStyle(.secondary) }
                else if member.canSetRole {
                    Picker("Role", selection: Binding(get: { member.viewer }, set: { viewer in
                        run("role") { try await bridge.action("folderSetRole", ["folderId": folderID, "pairId": member.pairId, "bool": viewer]) }
                    })) {
                        Text("Editor").tag(false)
                        Text("Viewer").tag(true)
                    }.pickerStyle(.segmented).frame(maxWidth: 200).accessibilityLabel("\(member.name)’s role")
                } else {
                    HStack(spacing: 6) {
                        Circle().fill(member.online ? Color.green : .secondary).frame(width: 6, height: 6)
                        Text((member.online ? "Online" : "Offline") + (member.viewer ? " · Viewer" : " · Editor")).font(.footnote).foregroundStyle(.secondary)
                    }
                }
            }
            Spacer(minLength: 0)
            Menu {
                if member.pending {
                    Button("Show Invite Code", systemImage: "qrcode") {
                        run("code") { let value: String = try await bridge.call("folderShowInvite", ["folderId": folderID, "pairId": member.pairId]); code = InviteCode(code: value, folderName: folder.name) }
                    }
                }
                Button(member.pending ? "Cancel Invite" : "Remove from Folder", systemImage: "person.badge.minus", role: .destructive) { removing = member }
            } label: { Image(systemName: "ellipsis").frame(width: 44, height: 44) }.accessibilityLabel("Options for \(member.name)")
        }.frame(minHeight: 60)
    }
    private func openInFiles() {
        let path = folder.path
        guard !path.isEmpty, var parts = URLComponents(url: URL(fileURLWithPath: path, isDirectory: true), resolvingAgainstBaseURL: false) else { return }
        parts.scheme = "shareddocuments"
        guard let url = parts.url else { return }
        UIApplication.shared.open(url) { opened in
            if !opened { Task { @MainActor in bridge.showToast("Open Files → On My iPhone → DropBeam to find this folder.") } }
        }
    }
    private func run(_ key: String, _ work: @escaping @MainActor () async throws -> Void) {
        busy = key
        bridge.perform { defer { busy = nil }; try await work() }
    }
}

struct InviteCode: Identifiable { var id: String { code }; let code: String; let folderName: String }

/// A folder invite as QR + text, like desktop's invite modal.
struct InviteCodeSheet: View {
    @Environment(\.dismiss) private var dismiss
    let invite: InviteCode
    @State private var copied = false
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(spacing: 20) {
                    Text("Send this to the person you want to invite. They scan or paste it in DropBeam to join \(invite.folderName).").font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
                    QRCodeView(code: invite.code)
                    Text(invite.code).font(.footnote.monospaced()).textSelection(.enabled).lineLimit(4).multilineTextAlignment(.center)
                    HStack(spacing: 12) {
                        Button { UIPasteboard.general.string = invite.code; copied = true; Haptics.tap() } label: { Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc").frame(maxWidth: .infinity, minHeight: 36) }.beamButton()
                        ShareLink(item: invite.code) { Label("Share", systemImage: "square.and.arrow.up").frame(maxWidth: .infinity, minHeight: 36) }.beamButton(prominent: true)
                    }
                }.padding(20)
            }.navigationTitle("Invite to Folder").navigationBarTitleDisplayMode(.inline).beamCanvas()
                .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }.tint(.beam)
    }
}

/// Invite existing friends straight into a folder (they get an accept prompt).
struct InviteFriendsSheet: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let folder: SharedFolder
    @State private var sending: String?
    @State private var sent: Set<String> = []
    @State private var error: String?
    private var candidates: [Friend] {
        let members = Set(folder.members.compactMap(\.friendId))
        return bridge.friends.filter { $0.groupedUnder == nil && !members.contains($0.id) && !($0.endpointId ?? "").isEmpty }
    }
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 16) {
                    Text("They’ll get a prompt to join \(folder.name) and choose where to keep it.").font(.subheadline).foregroundStyle(.secondary)
                    if candidates.isEmpty {
                        BeamEmpty(symbol: "person.2", title: "Everyone’s already here.", detail: "Add more friends from the Friends tab, or share an invite code instead.")
                    }
                    ForEach(candidates) { friend in
                        Button { invite(friend) } label: {
                            GlassCard {
                                HStack(spacing: 14) {
                                    ContactAvatar(friend: friend)
                                    VStack(alignment: .leading, spacing: 5) { Text(friend.displayName).font(.headline).foregroundStyle(.primary); PresenceLabel(online: bridge.presence[friend.id] == true) }
                                    Spacer()
                                    if sending == friend.id { ProgressView() }
                                    else if sent.contains(friend.id) { Label("Invited", systemImage: "checkmark").font(.footnote.weight(.semibold)).foregroundStyle(.green) }
                                    else { Image(systemName: "paperplane").foregroundStyle(.tint) }
                                }
                            }
                        }.buttonStyle(.plain).disabled(sending != nil || sent.contains(friend.id))
                    }
                    if let error { Text(error).foregroundStyle(.red).font(.subheadline) }
                }.padding(20)
            }.navigationTitle("Invite a Friend").navigationBarTitleDisplayMode(.inline).beamCanvas()
                .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }.tint(.beam)
    }
    private func invite(_ friend: Friend) {
        sending = friend.id; error = nil; Haptics.tap()
        Task {
            defer { sending = nil }
            do { try await bridge.action("folderInviteFriend", ["folderId": folder.id, "friendId": friend.id]); sent.insert(friend.id) }
            catch { self.error = error.localizedDescription }
        }
    }
}

/// Deleted or replaced files this folder kept (same engine data as History → Recoverable).
struct FolderRecoverableView: View {
    @EnvironmentObject private var bridge: Bridge
    let folderID: String
    let pairID: String
    let name: String
    @State private var items: [RecoveryItem] = []
    @State private var loading = true
    @State private var error: String?
    @State private var deleting: RecoveryItem?
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 16) {
                if let error { BeamError(message: error) { Task { await load() } } }
                if loading && items.isEmpty { ProgressView().frame(maxWidth: .infinity).padding(.vertical, 24) }
                else if items.isEmpty && error == nil { BeamEmpty(symbol: "clock.arrow.circlepath", title: "Nothing to recover.", detail: "When a file in \(name) is deleted or replaced, a copy waits here so you can bring it back.") }
                if !items.isEmpty {
                    GlassCard {
                        VStack(spacing: 0) {
                            ForEach(Array(items.enumerated()), id: \.element.id) { index, item in
                                if index > 0 { Divider().padding(.leading, 56) }
                                HStack(spacing: 12) {
                                    FileGlyph(name: item.relPath)
                                    VStack(alignment: .leading, spacing: 4) {
                                        Text((item.relPath as NSString).lastPathComponent).font(.headline).lineLimit(1)
                                        Text("\(Formatters.bytes(item.size)) · \(item.date.formatted(.relative(presentation: .named)))").font(.subheadline).foregroundStyle(.secondary)
                                    }
                                    Spacer(minLength: 0)
                                    Menu {
                                        Button("Restore", systemImage: "arrow.uturn.backward") { mutate("recoverableRestore", item) }
                                        Button("Delete Forever", systemImage: "trash", role: .destructive) { deleting = item }
                                    } label: { Image(systemName: "ellipsis").frame(width: 44, height: 44) }.accessibilityLabel("Options for \(item.relPath)")
                                }.frame(minHeight: 56)
                            }
                        }
                    }
                }
            }.padding(20)
        }
        .contentMargins(.bottom, 24, for: .scrollContent)
        .navigationTitle("Recoverable Files").navigationBarTitleDisplayMode(.inline).beamCanvas()
        .task { await load() }
        .refreshable { await load() }
        .onReceive(NotificationCenter.default.publisher(for: .init("DropBeam.folder-history://changed"))) { _ in Task { await load() } }
        .confirmationDialog("Delete this saved copy forever?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible) {
            if let item = deleting { Button("Delete Forever", role: .destructive) { mutate("recoverableForget", item) } }
        }
    }
    private func load() async {
        loading = true; error = nil
        defer { loading = false }
        do { items = try await bridge.call("recoverableItems", ["folder": pairID]) }
        catch { self.error = error.localizedDescription }
    }
    private func mutate(_ command: String, _ item: RecoveryItem) {
        bridge.perform {
            try await bridge.action(command, ["item": ["folder": pairID, "id": item.id]])
            bridge.showToast(command == "recoverableRestore" ? "File restored" : "Saved copy removed")
            await load()
        }
    }
}
