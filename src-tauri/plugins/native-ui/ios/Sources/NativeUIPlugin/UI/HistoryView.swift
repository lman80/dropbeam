import SwiftUI

struct HistoryView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var segment = 0
    @State private var search = ""
    @State private var clearing = false
    @State private var media: LocalMedia?
    private var filtered: [HistoryEntry] {
        bridge.history.filter { search.isEmpty || ($0.fileNames.joined(separator: " ") + " " + ($0.peer ?? "")).localizedCaseInsensitiveContains(search) }
    }
    private var days: [Date] { Set(filtered.map { Calendar.current.startOfDay(for: $0.date) }).sorted(by: >) }
    var body: some View {
        NavigationStack {
            Group {
                if segment == 0 { recents } else { RecoverableView(search: search) { picker } }
            }
            .navigationTitle("History").navigationBarTitleDisplayMode(.large)
            .searchable(text: $search, prompt: segment == 0 ? "Files or people" : "Saved copies")
            .toolbar { ToolbarItem(placement: .topBarTrailing) {
                if segment == 0 {
                    Menu { Button("Clear History", systemImage: "trash", role: .destructive) { clearing = true } } label: { Image(systemName: "ellipsis") }
                        .accessibilityLabel("History options").disabled(bridge.history.isEmpty)
                }
            } }
            .confirmationDialog("Clear all transfer history?", isPresented: $clearing, titleVisibility: .visible) {
                Button("Clear History", role: .destructive) { bridge.perform { try await bridge.action("historyClear") } }
            } message: { Text("Your files stay where they are.") }
            .task { // Tab appearance must not fire a haptic (perform taps); keep the error alert.
                do { try await bridge.action("historyList") } catch is CancellationError {} catch { bridge.errorMessage = error.localizedDescription }
            }
            .fullScreenCover(item: $media) { item in MediaViewer(path: item.path, name: item.name, video: item.video) }
        }
    }
    private var picker: some View {
        Picker("Show", selection: $segment) { Text("Recents").tag(0); Text("Recoverable").tag(1) }
            .pickerStyle(.segmented).clearRow(EdgeInsets(top: 0, leading: 20, bottom: 4, trailing: 20))
    }
    private var recents: some View {
        List {
            Section { picker }
            ForEach(days, id: \.self) { day in
                Section {
                    let entries = filtered.filter { Calendar.current.isDate($0.date, inSameDayAs: day) }.sorted { $0.timestampMs > $1.timestampMs }
                    ForEach(entries) { entry in
                        Button { open(entry) } label: { historyRow(entry) }.buttonStyle(.plain)
                            .swipeActions(edge: .trailing) {
                                Button(role: .destructive) { remove(entry) } label: { Label("Remove", systemImage: "trash") }
                            }
                            .swipeActions(edge: .leading) {
                                if !entry.localPaths.isEmpty { Button { share(entry) } label: { Label("Share", systemImage: "square.and.arrow.up") }.tint(.beam) }
                            }
                            .contextMenu {
                                Button("Share", systemImage: "square.and.arrow.up") { share(entry) }.disabled(entry.localPaths.isEmpty)
                                Button("Copy Name", systemImage: "doc.on.doc") { UIPasteboard.general.string = entry.fileNames.joined(separator: ", "); Haptics.tap() }
                                Button("Remove from History", systemImage: "trash", role: .destructive) { remove(entry) }
                            }
                    }
                } header: { Text(dayLabel(day)) }.headerProminence(.increased)
            }
        }
        .beamList()
        .overlay {
            if filtered.isEmpty {
                if search.isEmpty {
                    ContentUnavailableView("No Transfers Yet", systemImage: "clock.arrow.circlepath", description: Text("Everything you send and receive shows up here, ready to open or share again."))
                        .allowsHitTesting(false)
                } else { ContentUnavailableView.search(text: search) }
            }
        }
        .refreshable { try? await bridge.action("historyList") }
    }
    private func historyRow(_ entry: HistoryEntry) -> some View {
        HStack(spacing: 14) {
            Group {
                if let path = entry.localPaths.first, LocalMedia(path: path) != nil {
                    MediaThumbnail(path: path, width: 44, height: 44).clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
                } else { FileGlyph(name: entry.fileNames.first ?? "", symbol: entry.fileNames.count > 1 ? "doc.on.doc" : nil) }
            }
            .overlay(alignment: .bottomTrailing) {
                Image(systemName: entry.direction == "send" ? "arrow.up.circle.fill" : "arrow.down.circle.fill")
                    .font(.system(size: 15, weight: .bold)).symbolRenderingMode(.palette)
                    .foregroundStyle(.white, entry.state == "failed" ? Color.red : entry.direction == "send" ? Color.beam : .green)
                    .background(Circle().fill(Color(uiColor: .secondarySystemGroupedBackground)).padding(-2))
                    .offset(x: 5, y: 5).accessibilityHidden(true)
            }
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline) {
                    Text(entry.title).font(.body.weight(.semibold)).lineLimit(1).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                    Spacer(minLength: 6)
                    Text(entry.date.formatted(date: .omitted, time: .shortened)).font(.subheadline).foregroundStyle(.secondary)
                }
                Text("\(entry.direction == "send" ? "To" : "From") \(entry.peer ?? "a device") · \(Formatters.bytes(entry.bytesTotal)) · \(route(entry.locality))")
                    .font(.subheadline).foregroundStyle(.secondary).lineLimit(2)
                if entry.state == "failed" { Text(entry.error ?? "Transfer failed").font(.footnote).foregroundStyle(.red).lineLimit(2) }
            }
        }.padding(.vertical, 4).contentShape(Rectangle())
        .accessibilityElement(children: .combine)
        .accessibilityHint(entry.localPaths.isEmpty ? "" : "Opens the file")
    }
    private func route(_ value: String?) -> String {
        switch value { case "internet": return "Relay"; case "local": return "Local"; case "direct": return "Direct"; default: return value?.capitalized ?? "Unknown route" }
    }
    private func dayLabel(_ day: Date) -> String {
        let calendar = Calendar.current
        if calendar.isDateInToday(day) { return "Today" }
        if calendar.isDateInYesterday(day) { return "Yesterday" }
        if let week = calendar.date(byAdding: .day, value: -7, to: Date()), day > week { return day.formatted(.dateTime.weekday(.wide)) }
        return day.formatted(date: .abbreviated, time: .omitted)
    }
    private func remove(_ entry: HistoryEntry) { bridge.perform { try await bridge.action("historyRemove", ["entryId": entry.id]) } }
    private func share(_ entry: HistoryEntry) { bridge.perform { try await bridge.action("historyOpen", ["entryId": entry.id]) } }
    private func open(_ entry: HistoryEntry) {
        Haptics.tap()
        if entry.localPaths.count == 1, let path = entry.localPaths.first, FileManager.default.fileExists(atPath: path), let item = LocalMedia(path: path) { media = item }
        else { share(entry) }
    }
}

/// Saved copies of files that were deleted or replaced in shared folders.
struct RecoverableView<Header: View>: View {
    @EnvironmentObject private var bridge: Bridge
    let search: String
    @ViewBuilder var header: Header
    @State private var summaries: [RecoverySummary] = []
    @State private var items: [String: [RecoveryItem]] = [:]
    @State private var loading = true
    @State private var busy = false
    @State private var error: String?
    @State private var emptyFolder: RecoverySummary?
    @State private var deleting: RecoveryDeletion?
    @State private var emptyAll = false
    private var total: Double { summaries.reduce(0) { $0 + $1.bytes } }
    private var budget: Double { (bridge.settings?.folderHistoryBudgetBytes ?? 2147483648) * Double(summaries.count) }
    var body: some View {
        List {
            Section { header }
            if !summaries.isEmpty {
                Section {
                    VStack(alignment: .leading, spacing: 10) {
                        HStack(alignment: .firstTextBaseline) {
                            Text(Formatters.bytes(total)).font(.title2.weight(.semibold)).monospacedDigit()
                            Text(budget > 0 ? "of \(Formatters.bytes(budget))" : "· No storage limit").foregroundStyle(.secondary)
                        }
                        ProgressView(value: budget > 0 ? min(1, total / budget) : 0).tint(.beam)
                    }.padding(.vertical, 4).accessibilityElement(children: .combine)
                    Button("Empty All Saved Copies", role: .destructive) { emptyAll = true }
                } header: { Text("Storage") } footer: {
                    Text("Old copies are removed automatically. Your live files are never touched. The limit applies to each shared folder.")
                }
            }
            ForEach(summaries) { folder in
                Section {
                    let rows = (items[folder.id] ?? []).filter { search.isEmpty || $0.relPath.localizedCaseInsensitiveContains(search) }
                    if items[folder.id]?.isEmpty == true { Text("No saved copies in this folder.").foregroundStyle(.secondary) }
                    ForEach(rows) { item in
                        HStack(spacing: 14) {
                            FileGlyph(name: item.relPath)
                            VStack(alignment: .leading, spacing: 3) {
                                Text((item.relPath as NSString).lastPathComponent).font(.body.weight(.semibold)).lineLimit(1).alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                                Text("\(Formatters.bytes(item.size)) · \(item.date.formatted(.relative(presentation: .named)))").font(.subheadline).foregroundStyle(.secondary)
                            }
                            Spacer(minLength: 0)
                            Menu {
                                Button("Restore", systemImage: "arrow.uturn.backward") { mutate("recoverableRestore", folder: folder.id, item: item.id) }
                                Button("Delete Forever", systemImage: "trash", role: .destructive) { deleting = RecoveryDeletion(folder: folder.id, item: item) }
                            } label: { Image(systemName: "ellipsis.circle").font(.title3).frame(minWidth: 44, minHeight: 44) }
                                .buttonStyle(.borderless).accessibilityLabel("Options for \((item.relPath as NSString).lastPathComponent)")
                        }
                        .swipeActions(edge: .leading) { Button { mutate("recoverableRestore", folder: folder.id, item: item.id) } label: { Label("Restore", systemImage: "arrow.uturn.backward") }.tint(.green) }
                        .swipeActions(edge: .trailing) { Button(role: .destructive) { deleting = RecoveryDeletion(folder: folder.id, item: item) } label: { Label("Delete", systemImage: "trash") } }
                    }
                } header: {
                    HStack {
                        Text(folder.folderName)
                        Spacer()
                        Button("Empty") { emptyFolder = folder }.font(.body).textCase(nil).foregroundStyle(.red).accessibilityLabel("Empty \(folder.folderName)")
                    }
                }.headerProminence(.increased)
            }
        }
        .beamList()
        .overlay {
            if loading && summaries.isEmpty { ProgressView() }
            else if let error { BeamError(message: error) { Task { await load() } } }
            else if summaries.isEmpty {
                ContentUnavailableView("Nothing to Recover", systemImage: "externaldrive.badge.checkmark", description: Text("When a file in a shared folder is deleted or replaced, a saved copy waits here."))
                    .allowsHitTesting(false)
            }
        }
        .disabled(busy)
        .refreshable { await load() }
        .task { await load() }
        .onReceive(NotificationCenter.default.publisher(for: .init("DropBeam.folder-history://changed"))) { _ in Task { await load() } }
        .confirmationDialog("Empty \(emptyFolder?.folderName ?? "folder")?", isPresented: Binding(get: { emptyFolder != nil }, set: { if !$0 { emptyFolder = nil } }), titleVisibility: .visible) {
            if let folder = emptyFolder { Button("Empty Folder", role: .destructive) { mutate("recoverableEmpty", folder: folder.id) } }
        } message: { Text("Its saved copies will be deleted permanently.") }
        .confirmationDialog("Delete this saved copy forever?", isPresented: Binding(get: { deleting != nil }, set: { if !$0 { deleting = nil } }), titleVisibility: .visible) {
            if let deletion = deleting { Button("Delete Forever", role: .destructive) { mutate("recoverableForget", folder: deletion.folder, item: deletion.item.id) } }
        }
        .confirmationDialog("Permanently delete all saved copies?", isPresented: $emptyAll, titleVisibility: .visible) {
            Button("Empty All", role: .destructive) { mutate("recoverableEmptyAll", folder: "") }
        }
    }
    private func load() async {
        loading = true; error = nil
        defer { loading = false }
        do {
            let next: [RecoverySummary] = try await bridge.call("recoverableSummaries")
            var nextItems: [String: [RecoveryItem]] = [:]
            for folder in next { nextItems[folder.id] = try await bridge.call("recoverableItems", ["folder": folder.id]) }
            summaries = next; items = nextItems
        } catch is CancellationError {} catch { self.error = error.localizedDescription }
    }
    private func mutate(_ command: String, folder: String, item: String? = nil) {
        busy = true
        bridge.perform {
            defer { busy = false }
            try await bridge.action(command, item.map { ["item": ["folder": folder, "id": $0]] } ?? ["folder": folder])
            bridge.showToast(command == "recoverableRestore" ? "File restored" : "Saved copies removed")
            await load()
        }
    }
}
private struct RecoveryDeletion { let folder: String; let item: RecoveryItem }
