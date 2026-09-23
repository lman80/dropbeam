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
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    GlassCard { Picker("History", selection: $segment) { Text("Recents").tag(0); Text("Recoverable").tag(1) }.pickerStyle(.segmented) }
                    if segment == 0 { recents } else { RecoverableView(search: search) }
                }.padding(20)
            }.contentMargins(.bottom, 24, for: .scrollContent)
                .navigationTitle("History").navigationBarTitleDisplayMode(.large).beamCanvas().searchable(text: $search, prompt: "Find files or people")
                .toolbar { ToolbarItem(placement: .topBarTrailing) {
                    Menu { Button("Clear History", role: .destructive) { clearing = true } } label: { Image(systemName: "ellipsis").frame(width: 44, height: 44) }.accessibilityLabel("History options")
                } }
                .confirmationDialog("Clear all transfer history? Your files stay where they are.", isPresented: $clearing, titleVisibility: .visible) {
                    Button("Clear History", role: .destructive) { bridge.perform { try await bridge.action("historyClear") } }
                }
                .task { // Tab appearance must not fire a haptic (perform taps); keep the error alert.
                    do { try await bridge.action("historyList") } catch is CancellationError {} catch { bridge.errorMessage = error.localizedDescription }
                }
                .fullScreenCover(item: $media) { item in MediaViewer(path: item.path, name: item.name, video: item.video) }
        }
    }
    @ViewBuilder private var recents: some View {
        if filtered.isEmpty { BeamEmpty(symbol: "clock.arrow.circlepath", title: search.isEmpty ? "Every arrival has a story." : "No matches yet.", detail: "Your transfers will appear here, ready to revisit.") }
        ForEach(days, id: \.self) { day in
            VStack(alignment: .leading, spacing: 12) {
                Text(dayLabel(day)).font(.title2.weight(.semibold))
                GlassCard {
                    LazyVStack(spacing: 0) {
                        let entries = filtered.filter { Calendar.current.isDate($0.date, inSameDayAs: day) }.sorted { $0.timestampMs > $1.timestampMs }
                        ForEach(entries) { entry in
                            Button { open(entry) } label: { historyRow(entry) }.buttonStyle(.plain)
                                .contextMenu {
                                    Button("Share", systemImage: "square.and.arrow.up") { share(entry) }.disabled(entry.localPaths.isEmpty)
                                    Button("Copy Name", systemImage: "doc.on.doc") { UIPasteboard.general.string = entry.fileNames.joined(separator: ", "); Haptics.tap() }
                                    Button("Remove", systemImage: "trash", role: .destructive) { bridge.perform { try await bridge.action("historyRemove", ["entryId": entry.id]) } }
                                }
                            if entry.id != entries.last?.id { Divider() }
                        }
                    }
                }
            }
        }
    }
    private func historyRow(_ entry: HistoryEntry) -> some View {
        HStack(spacing: 14) {
            if let path = entry.localPaths.first, LocalMedia(path: path) != nil {
                MediaThumbnail(path: path, width: 48, height: 48).clipShape(RoundedRectangle(cornerRadius: 10))
            } else { FileGlyph(name: entry.fileNames.first ?? "") }
            VStack(alignment: .leading, spacing: 5) {
                Text(entry.title).font(.headline)
                Text("\(Formatters.bytes(entry.bytesTotal)) · \(entry.direction == "send" ? "To" : "From") \(entry.peer ?? "a device") · \(route(entry.locality))").font(.subheadline).foregroundStyle(.secondary)
                if entry.state == "failed" { Text(entry.error ?? "Transfer failed").font(.subheadline).foregroundStyle(.red) }
            }.frame(maxWidth: .infinity, alignment: .leading)
        }.padding(.vertical, 12).contentShape(Rectangle())
    }
    private func route(_ value: String?) -> String { value == "internet" ? "Relay" : value?.capitalized ?? "Unknown" }
    private func dayLabel(_ day: Date) -> String {
        let calendar = Calendar.current
        if calendar.isDateInToday(day) { return "Today" }
        if calendar.isDateInYesterday(day) { return "Yesterday" }
        if let week = calendar.date(byAdding: .day, value: -7, to: Date()), day > week { return day.formatted(.dateTime.weekday(.wide)) }
        return day.formatted(date: .abbreviated, time: .omitted)
    }
    private func share(_ entry: HistoryEntry) { bridge.perform { try await bridge.action("historyOpen", ["entryId": entry.id]) } }
    private func open(_ entry: HistoryEntry) {
        Haptics.tap()
        if entry.localPaths.count == 1, let path = entry.localPaths.first, FileManager.default.fileExists(atPath: path), let item = LocalMedia(path: path) { media = item }
        else { share(entry) }
    }
}

struct RecoverableView: View {
    @EnvironmentObject private var bridge: Bridge
    let search: String
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
        VStack(alignment: .leading, spacing: 24) {
            if loading { ProgressView().frame(maxWidth: .infinity) }
            if let error { BeamError(message: error) { Task { await load() } } }
            if !loading && summaries.isEmpty && error == nil { BeamEmpty(symbol: "externaldrive", title: "Nothing to recover.", detail: "Saved copies of deleted or replaced files will be waiting here.") }
            if !summaries.isEmpty {
                GlassCard {
                    VStack(alignment: .leading, spacing: 14) {
                        Text("Room to go back").font(.title2.weight(.semibold))
                        Text(budget > 0 ? "\(Formatters.bytes(total)) of \(Formatters.bytes(budget))" : "\(Formatters.bytes(total)) · No storage limit").font(.headline)
                        ProgressView(value: budget > 0 ? min(1, total / budget) : 0).tint(.beam).clipShape(Capsule())
                        Text("Old copies are removed automatically. Your live files stay untouched. The storage limit applies to each folder.").font(.subheadline).foregroundStyle(.secondary)
                        Button("Empty All", role: .destructive) { emptyAll = true }.beamButton()
                    }
                }
            }
            ForEach(summaries) { folder in
                GlassCard {
                    VStack(alignment: .leading, spacing: 12) {
                        HStack { Text(folder.folderName).font(.headline); Spacer(); Button("Empty", role: .destructive) { emptyFolder = folder }.frame(minHeight: 44) }
                        ForEach((items[folder.id] ?? []).filter { search.isEmpty || $0.relPath.localizedCaseInsensitiveContains(search) }) { item in
                            Divider()
                            HStack(spacing: 12) {
                                FileGlyph(name: item.relPath)
                                VStack(alignment: .leading, spacing: 4) {
                                    Text((item.relPath as NSString).lastPathComponent).font(.headline)
                                    Text("\(Formatters.bytes(item.size)) · \(item.date.formatted(.relative(presentation: .named)))").font(.subheadline).foregroundStyle(.secondary)
                                }
                                Spacer(minLength: 0)
                                Menu {
                                    Button("Restore", systemImage: "arrow.uturn.backward") { mutate("recoverableRestore", folder: folder.id, item: item.id) }
                                    Button("Delete Forever", systemImage: "trash", role: .destructive) { deleting = RecoveryDeletion(folder: folder.id, item: item) }
                                } label: { Image(systemName: "ellipsis").frame(width: 44, height: 44) }.accessibilityLabel("Options for \(item.relPath)")
                            }
                        }
                        if items[folder.id]?.isEmpty == true { Text("No saved copies in this folder.").foregroundStyle(.secondary) }
                    }
                }
            }
        }.disabled(busy).task { await load() }
            .onReceive(NotificationCenter.default.publisher(for: .init("DropBeam.folder-history://changed"))) { _ in Task { await load() } }
            .confirmationDialog("Empty \(emptyFolder?.folderName ?? "folder")? Saved copies will be deleted permanently.", isPresented: Binding(get: { emptyFolder != nil }, set: { if !$0 { emptyFolder = nil } }), titleVisibility: .visible) {
                if let folder = emptyFolder { Button("Empty Folder", role: .destructive) { mutate("recoverableEmpty", folder: folder.id) } }
            }
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
        } catch { self.error = error.localizedDescription }
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

struct FileGlyph: View {
    let name: String
    var symbol: String? = nil
    var body: some View { Image(systemName: symbol ?? Formatters.symbol(name)).font(.title2).foregroundStyle(symbol == "folder.fill" ? Color.blue : .beam).frame(width: 44, height: 44).background((symbol == "folder.fill" ? Color.blue : .beam).opacity(0.12), in: RoundedRectangle(cornerRadius: 12)).accessibilityHidden(true) }
}
struct BeamEmpty: View {
    let symbol: String
    let title: String
    let detail: String
    var body: some View {
        GlassCard { VStack(alignment: .leading, spacing: 20) { Image(systemName: symbol).font(.system(size: 48, weight: .light)).foregroundStyle(.tint); Text(title).font(.title2.weight(.semibold)); Text(detail).foregroundStyle(.secondary) }.padding(.vertical, 16) }
    }
}
struct BeamError: View {
    let message: String
    let retry: () -> Void
    var body: some View { GlassCard { VStack(alignment: .leading, spacing: 14) { Label("Couldn’t load this", systemImage: "wifi.exclamationmark").font(.headline); Text(message).foregroundStyle(.secondary); Button("Try Again", action: retry).beamButton() } } }
}
