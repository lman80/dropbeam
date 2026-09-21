import SwiftUI

struct LocationsView: View {
    @EnvironmentObject private var bridge: Bridge
    var friendID: String? = nil
    @State private var loading = false
    @State private var error: String?
    private var groups: [FriendLocations] { bridge.locations.filter { friendID == nil || $0.friendId == friendID } }
    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: 24) {
                if loading { ProgressView().frame(maxWidth: .infinity) }
                if let error { BeamError(message: error, retry: refresh) }
                if !loading && groups.allSatisfy({ $0.locations.isEmpty }) { BeamEmpty(symbol: "externaldrive", title: "A place for everything.", detail: "Folders shared by friends appear here. Keep their device awake to browse.") }
                ForEach(groups) { friend in
                    VStack(alignment: .leading, spacing: 12) {
                        HStack { Text(friend.friendName).font(.title2.weight(.semibold)); Spacer(); PresenceLabel(online: friend.online) }
                        if let error = friend.error { BeamError(message: error, retry: refresh) }
                        ForEach(friend.locations) { location in
                            NavigationLink { BrowserView(friendID: friend.friendId, location: location, path: "") } label: {
                                GlassCard {
                                    HStack(spacing: 14) {
                                        FileGlyph(name: "", symbol: "externaldrive")
                                        VStack(alignment: .leading, spacing: 5) {
                                            Text(location.name).font(.headline).foregroundStyle(.primary)
                                            Text("\(friend.friendName) · \(friend.online ? "Online now" : "Offline")").font(.subheadline).foregroundStyle(.secondary)
                                            if location.reachable == false { Text("Location unavailable").font(.caption).foregroundStyle(.secondary) }
                                        }
                                        Spacer(minLength: 0); Image(systemName: "chevron.right").foregroundStyle(.tertiary)
                                    }
                                }
                            }.buttonStyle(.plain)
                        }
                    }
                }
            }.padding(20)
        }.contentMargins(.bottom, 24, for: .scrollContent).navigationTitle("Locations").navigationBarTitleDisplayMode(.large).beamCanvas()
            .toolbar { ToolbarItem(placement: .topBarTrailing) { Button(action: refresh) { Image(systemName: "arrow.clockwise").frame(width: 44, height: 44) }.accessibilityLabel("Refresh locations").disabled(loading) } }
            .task { refresh() }
            .refreshable { await reload() }
            .onReceive(NotificationCenter.default.publisher(for: .init("DropBeam.locations://changed"))) { _ in refresh() }
    }
    private func refresh() { guard !loading else { return }; Haptics.tap(); Task { await reload() } }
    private func reload() async {
        loading = true; error = nil
        defer { loading = false }
        do { try await bridge.action("locationsRefresh") } catch { self.error = error.localizedDescription }
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
        ScrollView {
            VStack(spacing: 18) {
                if let banner { GlassCard { Text(banner).font(.subheadline).accessibilityAddTraits(.updatesFrequently) } }
                if let error { BeamError(message: error) { Task { await load() } } }
                if loading { ProgressView().frame(maxWidth: .infinity).padding() }
                if !loading && error == nil && page.entries.isEmpty { BeamEmpty(symbol: "folder", title: query.isEmpty ? "This folder is empty." : "No matching files.", detail: query.isEmpty ? "Files shared here will appear in this folder." : "Try another name in this folder.") }
                if !page.entries.isEmpty {
                    GlassCard {
                        LazyVStack(spacing: 0) {
                            ForEach(page.entries) { entry in
                                browserRow(entry)
                                if entry.id != page.entries.last?.id { Divider() }
                            }
                            if page.hasMore {
                                Button("Show More") { Task { await load(more: true) } }.frame(maxWidth: .infinity, minHeight: 44).disabled(loading)
                            }
                        }
                    }
                }
            }.padding(20)
        }.contentMargins(.bottom, 24, for: .scrollContent).navigationTitle(title).navigationBarTitleDisplayMode(.inline).beamCanvas()
            .searchable(text: $query, prompt: "Find in this folder")
            .task(id: query) {
                if !query.isEmpty { try? await Task.sleep(for: .milliseconds(250)) }
                guard !Task.isCancelled else { return }
                await load()
            }
            .toolbar { ToolbarItemGroup(placement: .topBarTrailing) {
                Button(selecting ? "Done" : "Select") { Haptics.tap(); selecting.toggle(); selected.removeAll() }.disabled(busy)
                Menu {
                    if rights.manage { Button("New Folder", systemImage: "folder.badge.plus") { name = ""; editing = "mkdir" } }
                    if rights.upload {
                        Button("Upload Photos", systemImage: "photo") { upload("photos") }
                        Button("Upload Files", systemImage: "doc") { upload("files") }
                        Button("Upload Folder", systemImage: "folder") { upload("folder") }
                    }
                    Button("Refresh", systemImage: "arrow.clockwise") { Task { await load() } }
                } label: { Image(systemName: "ellipsis").frame(width: 44, height: 44) }.accessibilityLabel("Folder options").disabled(busy || loading)
            } }
            .safeAreaInset(edge: .bottom) { if selecting { selectionBar.padding(.horizontal, 20).padding(.bottom, 8) } }
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
        if entry.isDir && !selecting {
            NavigationLink { BrowserView(friendID: friendID, location: location, path: child(entry.name)) } label: { rowContent(entry) }.buttonStyle(.plain).disabled(busy)
        } else {
            Button {
                Haptics.tap()
                if selecting { if !selected.insert(entry.name).inserted { selected.remove(entry.name) } }
                else { tapped = entry }
            } label: { rowContent(entry) }.buttonStyle(.plain).disabled(busy)
        }
    }
    private func rowContent(_ entry: BrowserEntry) -> some View {
        HStack(spacing: 14) {
            if selecting { Image(systemName: selected.contains(entry.name) ? "checkmark.circle.fill" : "circle").foregroundStyle(.tint).frame(width: 28, height: 44).accessibilityLabel(selected.contains(entry.name) ? "Selected" : "Not selected") }
            FileGlyph(name: entry.name, symbol: entry.isDir ? "folder.fill" : nil)
            VStack(alignment: .leading, spacing: 5) {
                Text(entry.name).font(.headline).foregroundStyle(.primary)
                if !entry.isDir { Text("\(Formatters.bytes(entry.size)) · \(entry.date.formatted(.dateTime.month(.abbreviated).day()))").font(.subheadline).foregroundStyle(.secondary) }
            }.frame(maxWidth: .infinity, alignment: .leading)
            if entry.isDir && !selecting { Image(systemName: "chevron.right").foregroundStyle(.tertiary) }
        }.padding(.vertical, 12).contentShape(Rectangle())
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
