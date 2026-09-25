import SwiftUI

/// Friends' shared folders (NAS mounts, disks, local folders). Friends that
/// share something get their own section; everyone else is summarised in one
/// quiet card that says exactly why nothing is listed for them (nothing shared
/// with this iPhone, offline, still checking, or what went wrong).
struct LocationsView: View {
    @EnvironmentObject private var bridge: Bridge
    var friendID: String? = nil
    @State private var loading = false
    @State private var refreshAgain = false
    @State private var failure: String?
    @State private var onlineBefore: Set<String> = []
    private var rows: [FriendLocations] { bridge.locations.filter { friendID == nil || $0.friendId == friendID } }
    private var sharing: [FriendLocations] { rows.filter { !$0.locations.isEmpty } }
    private var others: [FriendLocations] { rows.filter { $0.locations.isEmpty } }
    private var busy: Bool { loading || rows.contains(where: \.checking) }
    private var settled: Bool { !rows.isEmpty && rows.allSatisfy { !$0.checking && $0.status != "pending" } }
    var body: some View {
        List {
            ForEach(sharing) { friend in section(friend) }
            if friendID == nil && !others.isEmpty && !bridge.friends.isEmpty { otherFriends }
        }
        .beamList()
        .overlay {
            if let failure, sharing.isEmpty { BeamError(message: failure, retry: refresh) }
            else if friendID != nil, let friend = rows.first, friend.locations.isEmpty { friendState(friend) }
            else if bridge.friends.isEmpty && friendID == nil {
                ContentUnavailableView("No Friends Yet", systemImage: "person.2", description: Text("Add a friend who shares a folder or NAS, and it shows up here."))
            } else if sharing.isEmpty && others.isEmpty {
                if settled {
                    ContentUnavailableView("No Shared Locations", systemImage: "externaldrive", description: Text("When a friend shares a folder or NAS with this iPhone, it appears here. They set it up in DropBeam on their computer: Settings → Locations."))
                } else { ProgressView("Looking for shared folders…") }
            }
        }
        .navigationTitle(friendID == nil ? "Locations" : (rows.first?.friendName ?? "Locations")).navigationBarTitleDisplayMode(friendID == nil ? .large : .inline)
            .toolbar { ToolbarItem(placement: .topBarTrailing) {
                // Refreshing is automatic (and pull-to-refresh); just show when it's working.
                if busy { ProgressView().accessibilityLabel("Checking locations") }
            } }
            .task {
                onlineBefore = Set(bridge.presence.filter(\.value).map(\.key))
                // Coming back from a folder shouldn't re-check everyone; a stale
                // or never-checked list should.
                let now = Date().timeIntervalSince1970 * 1000
                if rows.isEmpty || rows.contains(where: { $0.status == "pending" || $0.status == "error" || now - ($0.checkedAt ?? 0) > 30_000 }) { await reload() }
            }
            .onChange(of: bridge.friends.map(\.id)) { _, _ in refresh() }
            .onChange(of: bridge.presence) { _, presence in
                // Only a friend COMING online changes what can be listed.
                let online = Set(presence.filter(\.value).map(\.key))
                let arrived = online.subtracting(onlineBefore)
                onlineBefore = online
                if rows.contains(where: { arrived.contains($0.friendId) && $0.status != "ready" }) { refresh() }
            }
            .refreshable { await reload() }
            .onReceive(NotificationCenter.default.publisher(for: .init("DropBeam.locations://changed"))) { _ in refresh() }
    }
    private func section(_ friend: FriendLocations) -> some View {
        Section {
            if friend.status == "offline" {
                Label("Offline — showing what was shared last time", systemImage: "moon.zzz.fill").font(.footnote).foregroundStyle(.secondary)
            } else if friend.status == "error", let error = friend.error {
                problem(error)
            }
            ForEach(friend.locations) { location in card(location, friend: friend) }
        } header: {
            if friendID == nil {
                HStack(alignment: .center) {
                    Text(friend.friendName).lineLimit(1)
                    Spacer(minLength: 8)
                    // After a failed request the error line is the truth, not a stale "Online".
                    if friend.checking { ProgressView().controlSize(.small) } else if friend.status != "error" { PresenceLabel(online: friend.online).textCase(nil) }
                }.accessibilityElement(children: .combine).accessibilityAddTraits(.isHeader)
            }
        }.headerProminence(.increased)
    }
    private func card(_ location: SharedLocation, friend: FriendLocations) -> some View {
        NavigationLink { BrowserView(friendID: friend.friendId, location: location, path: "") } label: {
            HStack(spacing: 14) {
                RowIcon(symbol: "externaldrive.fill", color: .teal)
                VStack(alignment: .leading, spacing: 3) {
                    Text(location.name).font(.body.weight(.semibold)).foregroundStyle(.primary).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                    Text(rights(location)).font(.subheadline).foregroundStyle(.secondary)
                    if location.reachable == false {
                        Label("Not reachable on \(friend.friendName) right now", systemImage: "exclamationmark.triangle.fill").font(.caption).foregroundStyle(.orange)
                    } else if friend.status == "ready", let free = location.freeBytes, let total = location.totalBytes, total > 0 {
                        Text("\(Formatters.bytes(free)) free of \(Formatters.bytes(total))").font(.caption).foregroundStyle(.secondary)
                    }
                }
            }.padding(.vertical, 2)
        }.opacity(friend.status == "offline" || friend.status == "error" ? 0.6 : 1).accessibilityHint("Browse this folder")
    }
    private func rights(_ location: SharedLocation) -> String {
        location.rights.manage ? "Download, upload & manage" : location.rights.upload ? "Download & upload" : "View & download"
    }
    private func problem(_ message: String) -> some View {
        HStack(alignment: .top, spacing: 10) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(.orange).accessibilityHidden(true)
            Text(message).font(.footnote).foregroundStyle(.secondary).frame(maxWidth: .infinity, alignment: .leading)
            Button("Retry", action: refresh).font(.footnote.weight(.semibold)).buttonStyle(.borderless).disabled(busy)
        }
    }
    /// Why a friend has nothing listed, in one line.
    private func reason(_ friend: FriendLocations) -> String {
        if friend.checking && friend.status == "pending" { return "Checking…" }
        switch friend.status {
        case "ready": return "Hasn’t shared a folder with this iPhone"
        case "offline": return "Offline — open DropBeam on it to check"
        case "pending": return "Not checked yet"
        default: return friend.error ?? "Couldn’t check"
        }
    }
    private var otherFriends: some View {
        Section(sharing.isEmpty ? "Your Friends" : "Other Friends") {
            ForEach(others) { friend in
                HStack(spacing: 12) {
                    if let known = bridge.friends.first(where: { $0.id == friend.friendId }) { ContactAvatar(friend: known, size: 36) }
                    VStack(alignment: .leading, spacing: 2) {
                        Text(friend.friendName).font(.body.weight(.medium)).lineLimit(1).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                        Text(reason(friend)).font(.footnote).foregroundStyle(friend.status == "error" ? Color.orange : .secondary).fixedSize(horizontal: false, vertical: true)
                    }.frame(maxWidth: .infinity, alignment: .leading)
                    if friend.checking { ProgressView().controlSize(.small) }
                    else if friend.status == "error" || friend.status == "offline" { Button("Retry", action: refresh).font(.footnote.weight(.semibold)).buttonStyle(.borderless).disabled(busy) }
                }.padding(.vertical, 2).accessibilityElement(children: .combine)
            }
        }
    }
    /// One friend's page ("Browse Locations" on their card) with nothing listed.
    @ViewBuilder private func friendState(_ friend: FriendLocations) -> some View {
        let retry = Button("Try Again", action: refresh).beamButton().disabled(busy)
        switch friend.status {
        case "ready":
            ContentUnavailableView { Label("Nothing Shared Yet", systemImage: "externaldrive") } description: { Text("\(friend.friendName) hasn’t shared a folder with this iPhone. They can share one in DropBeam on their computer: Settings → Locations.") } actions: { retry }
        case "offline":
            ContentUnavailableView { Label("\(friend.friendName) is Offline", systemImage: "moon.zzz") } description: { Text("Open DropBeam on \(friend.friendName) to see the folders it shares.") } actions: { retry }
        case "error", "unavailable":
            ContentUnavailableView { Label("Couldn’t Check", systemImage: "exclamationmark.triangle") } description: { Text(friend.error ?? "Something went wrong.") } actions: { retry }
        default:
            ProgressView("Looking for shared folders…")
        }
    }
    private func refresh() { Task { await reload() } }
    private func reload() async {
        guard !loading else { refreshAgain = true; return }
        loading = true; failure = nil
        defer { loading = false }
        repeat {
            refreshAgain = false
            do {
                // The reply is the snapshot with every asked friend marked `checking`;
                // results then stream in per friend. Wait for them (bounded) so
                // pull-to-refresh ends when the list is actually fresh.
                let started: LossyArray<FriendLocations> = try await bridge.call("locationsRefresh")
                if !started.values.isEmpty { bridge.locations = started.values }
                let deadline = Date().addingTimeInterval(90)
                while bridge.locations.contains(where: \.checking), Date() < deadline, !Task.isCancelled {
                    try await Task.sleep(for: .milliseconds(300))
                }
            } catch is CancellationError {} catch { failure = error.localizedDescription }
        } while refreshAgain
    }
}

struct BrowserView: View {
    @EnvironmentObject private var bridge: Bridge
    let friendID: String
    let location: SharedLocation
    let path: String
    @State private var query = ""
    @State private var page = BrowserPage()
    @State private var loading = true
    @State private var busy = false
    @State private var error: String?
    @State private var selected = Set<String>()
    @State private var selecting = false
    @State private var tapped: BrowserEntry?
    @State private var editing: String?
    @State private var name = ""
    @State private var trashConfirm = false
    @State private var banner: String?
    @State private var generation = 0
    private var currentLocation: SharedLocation? { bridge.locations.first { $0.friendId == friendID }?.locations.first { $0.id == location.id } }
    private var rights: LocationRights { currentLocation?.rights ?? LocationRights(upload: false, manage: false) }
    private var args: [String: Any] { ["friendId": friendID, "locationId": location.id, "path": path] }
    private var title: String { selecting ? "\(selected.count) selected" : path.isEmpty ? location.name : (path as NSString).lastPathComponent }
    var body: some View {
        List {
            if let error, !page.entries.isEmpty {
                Section { Label(error, systemImage: "exclamationmark.triangle.fill").font(.subheadline).foregroundStyle(.orange) }
            }
            if !page.entries.isEmpty {
                Section {
                    ForEach(page.entries) { entry in browserRow(entry) }
                    if page.hasMore {
                        Button("Show More") { Task { await load(more: true) } }.frame(maxWidth: .infinity, minHeight: 44).disabled(loading)
                    }
                } footer: { if let total = page.total, total > 0 { Text(total == 1 ? "1 item" : "\(total) items") } }
            }
        }
        .beamList()
        .overlay {
            if let error, page.entries.isEmpty {
                ContentUnavailableView { Label("Couldn’t Open This Folder", systemImage: "exclamationmark.triangle") } description: { Text(error) } actions: { Button("Try Again") { Task { await load() } }.beamButton() }
            } else if loading && page.entries.isEmpty { ProgressView() }
            else if !loading && page.entries.isEmpty {
                if query.isEmpty { ContentUnavailableView("Empty Folder", systemImage: "folder", description: Text(rights.upload ? "Upload photos or files with the ••• menu." : "Nothing here yet.")) }
                else { ContentUnavailableView.search(text: query) }
            }
        }
        .navigationTitle(title).navigationBarTitleDisplayMode(.inline)
            .searchable(text: $query, prompt: "Find in this folder")
            .refreshable { await load() }
            .task(id: query) {
                if !query.isEmpty { try? await Task.sleep(for: .milliseconds(250)) }
                guard !Task.isCancelled else { return }
                await load()
            }
            .toolbar { ToolbarItemGroup(placement: .topBarTrailing) {
                Button(selecting ? "Done" : "Select") { Haptics.tap(); selecting.toggle(); selected.removeAll() }.disabled(busy)
                if rights.manage || rights.upload { Menu {
                    if rights.manage { Button("New Folder", systemImage: "folder.badge.plus") { name = ""; editing = "mkdir" } }
                    if rights.upload {
                        Button("Upload Photos", systemImage: "photo") { upload("photos") }
                        Button("Upload Files", systemImage: "doc") { upload("files") }
                        Button("Upload Folder", systemImage: "folder") { upload("folder") }
                    }
                } label: { Image(systemName: "ellipsis") }.accessibilityLabel("Folder options").disabled(busy || loading) }
            } }
            .safeAreaInset(edge: .bottom) {
                VStack(spacing: 8) {
                    // Floats above the list so rows never jump under a finger when it appears or times out.
                    if let banner { Text(banner).font(.subheadline).padding(.horizontal, 18).padding(.vertical, 12).glassCapsule().accessibilityAddTraits(.updatesFrequently).transition(.move(edge: .bottom).combined(with: .opacity)) }
                    if selecting { selectionBar }
                }.padding(.horizontal, 20).padding(.bottom, 8).animation(.snappy, value: banner)
            }
            .confirmationDialog(tapped?.name ?? "File", isPresented: Binding(get: { tapped != nil }, set: { if !$0 { tapped = nil } }), titleVisibility: .visible) {
                if let entry = tapped {
                    Button("Download") { selected = [entry.name]; download() }
                    if rights.manage {
                        Button("Rename") { selected = [entry.name]; name = entry.name; editing = "rename" }
                        Button("Move to Trash", role: .destructive) { selected = [entry.name]; trashConfirm = true }
                    }
                }
            }
            .alert(editing == "mkdir" ? "New Folder" : "Rename", isPresented: Binding(get: { editing != nil }, set: { if !$0 { editing = nil } })) {
                let command = editing == "mkdir" ? "browserMkdir" : "browserRename"
                TextField("Name", text: $name).textInputAutocapitalization(.never).autocorrectionDisabled()
                Button("Cancel", role: .cancel) {}
                Button("Save") { saveName(command) }.disabled(name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
            }
            .confirmationDialog("Move \(selected.count) item(s) to Trash?", isPresented: $trashConfirm, titleVisibility: .visible) { Button("Move to Trash", role: .destructive, action: trash) }
            .onReceive(NotificationCenter.default.publisher(for: .init("DropBeam.locations://changed"))) { _ in if !busy { Task { await load() } } }
            .onChange(of: bridge.transfers.filter { $0.direction == "send" && $0.state == "completed" }.map(\.id)) { _, _ in if !busy { Task { await load() } } }
    }
    @ViewBuilder private func browserRow(_ entry: BrowserEntry) -> some View {
        Group {
            if entry.isDir && !selecting {
                NavigationLink { BrowserView(friendID: friendID, location: location, path: child(entry.name)) } label: { rowContent(entry) }.disabled(busy)
            } else {
                Button {
                    Haptics.tap()
                    if selecting { if !selected.insert(entry.name).inserted { selected.remove(entry.name) } }
                    else { tapped = entry }
                } label: { rowContent(entry) }.buttonStyle(.plain).disabled(busy)
            }
        }
        .swipeActions(edge: .leading) {
            if !selecting && !entry.isDir { Button { selected = [entry.name]; download() } label: { Label("Download", systemImage: "arrow.down.circle") }.tint(.beam) }
        }
        .swipeActions(edge: .trailing) {
            if !selecting && rights.manage {
                Button(role: .destructive) { selected = [entry.name]; trashConfirm = true } label: { Label("Trash", systemImage: "trash") }
                Button { selected = [entry.name]; name = entry.name; editing = "rename" } label: { Label("Rename", systemImage: "pencil") }.tint(.orange)
            }
        }
        .contextMenu {
            if !selecting {
                if !entry.isDir { Button("Download", systemImage: "arrow.down.circle") { selected = [entry.name]; download() } }
                if rights.manage {
                    Button("Rename", systemImage: "pencil") { selected = [entry.name]; name = entry.name; editing = "rename" }
                    Button("Move to Trash", systemImage: "trash", role: .destructive) { selected = [entry.name]; trashConfirm = true }
                }
            }
        }
    }
    private func rowContent(_ entry: BrowserEntry) -> some View {
        HStack(spacing: 14) {
            if selecting { Image(systemName: selected.contains(entry.name) ? "checkmark.circle.fill" : "circle").font(.title3).foregroundStyle(selected.contains(entry.name) ? Color.beam : Color.secondary).frame(width: 28, height: 44).accessibilityLabel(selected.contains(entry.name) ? "Selected" : "Not selected") }
            FileGlyph(name: entry.name, symbol: entry.isDir ? "folder.fill" : nil, size: 40)
            VStack(alignment: .leading, spacing: 3) {
                Text(entry.name).font(.body).foregroundStyle(.primary).lineLimit(2).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                if !entry.isDir { Text("\(Formatters.bytes(entry.size)) · \(entry.date.formatted(.dateTime.month(.abbreviated).day().year()))").font(.subheadline).foregroundStyle(.secondary) }
            }.frame(maxWidth: .infinity, alignment: .leading)
        }.padding(.vertical, 2).contentShape(Rectangle())
    }
    private var selectionBar: some View {
        GlassCard {
            ViewThatFits(in: .horizontal) {
                HStack(spacing: 16) { selectionActions }
                VStack(alignment: .leading, spacing: 8) { selectionActions }
            }.font(.subheadline.weight(.semibold)).disabled(busy || loading || error != nil)
        }
    }
    @ViewBuilder private var selectionActions: some View {
        Button(action: download) { Label("Download", systemImage: "arrow.down.circle").frame(minHeight: 44) }.disabled(selected.isEmpty)
        if rights.manage {
            Button { name = selected.first ?? ""; editing = "rename" } label: { Image(systemName: "pencil").frame(minWidth: 44, minHeight: 44) }.accessibilityLabel("Rename").disabled(selected.count != 1)
            Button(role: .destructive, action: { trashConfirm = true }) { Image(systemName: "trash").frame(minWidth: 44, minHeight: 44) }.accessibilityLabel("Trash").disabled(selected.isEmpty)
        }
    }
    private func child(_ name: String) -> String { path.isEmpty ? name : "\(path)/\(name)" }
    private func load(more: Bool = false) async {
        generation += 1; let token = generation
        loading = true; error = nil
        defer { if token == generation { loading = false } }
        var request = args; request["query"] = query
        if more { request["cursor"] = page.cursor }
        do {
            let next: BrowserPage = try await bridge.call("browserList", request)
            guard token == generation, !Task.isCancelled else { return }
            if more {
                let known = Set(page.entries.map(\.name))
                page = BrowserPage(entries: page.entries + next.entries.filter { !known.contains($0.name) }, hasMore: next.hasMore, cursor: next.cursor, total: next.total)
            } else { page = next }
            selected.formIntersection(Set(page.entries.map(\.name)))
        } catch {
            if token == generation && !Task.isCancelled {
                self.error = error.localizedDescription
                if !more { page = BrowserPage(); selected.removeAll() }
            }
        }
    }
    private func run(_ action: @escaping () async throws -> Void) {
        guard !busy else { return }; busy = true
        bridge.perform { defer { busy = false }; try await action() }
    }
    private func upload(_ source: String) {
        var request = args; request["source"] = source
        run { try await bridge.browserUpload(request); await load() }
    }
    private func download() {
        var request = args; request["names"] = Array(selected)
        run {
            let result: DownloadResult = try await bridge.call("browserDownload", request)
            let skipped = result.skipped ?? []
            showBanner(skipped.isEmpty ? (result.transferId == nil ? "No transferable files in this selection." : "Download started. Follow progress in Send.") : "Skipped: \(skipped.joined(separator: ", "))")
        }
    }
    private func saveName(_ command: String) {
        var request = args; request["name"] = name; request["to"] = name; request["from"] = selected.first ?? ""
        run { try await bridge.action(command, request); selected.removeAll(); await load(); showBanner(command == "browserMkdir" ? "Folder created" : "Renamed") }
    }
    private func trash() {
        var request = args; request["names"] = Array(selected)
        run {
            let result: [TrashResult] = try await bridge.call("browserTrash", request)
            let failed = result.filter { $0.error != nil }
            selected = Set(failed.map(\.name))
            await load()
            showBanner("\(result.count - failed.count) moved to Trash; \(failed.count) failed." + failed.map { " \($0.name): \($0.error ?? "")" }.joined())
        }
    }
    private func showBanner(_ text: String) {
        banner = text
        Task { try? await Task.sleep(for: .seconds(7)); if banner == text { banner = nil } }
    }
}
