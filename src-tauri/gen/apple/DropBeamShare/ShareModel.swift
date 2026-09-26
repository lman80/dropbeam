import Foundation
import UIKit
import UniformTypeIdentifiers
import QuickLookThumbnailing
import UserNotifications

/// Shared with the app through the App Group. The app half (ShareInbox.swift in the
/// native-ui plugin) decodes the same shapes — keep both in sync.
enum ShareGroup {
    static let id = "group.com.ashtonmiller.dropbeam"
    static var container: URL? { FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: id) }

    struct Recipient: Codable, Identifiable, Equatable {
        let id: String
        let name: String
        let own: Bool
        let deviceKind: String?
        let deviceOs: String?
        let online: Bool
        let avatar: String?
    }
    struct Snapshot: Codable {
        var version = 1
        let updatedMs: Double
        let recipients: [Recipient]
    }
    struct Job: Codable {
        var version = 1
        let id: String
        let createdMs: Double
        /// "friend" | "quick" | "choose"
        let recipient: String
        let friendId: String?
        let friendName: String?
        let files: [String]
        let texts: [String]
    }
}

@MainActor
final class ShareModel: ObservableObject {
    enum Outcome { case cancelled, done, open(URL) }
    enum Phase: Equatable {
        case preparing
        case ready
        /// Waiting for copies to finish / writing the job (recipient id, or "quick").
        case sending(String)
        /// The job is saved but iOS wouldn't open DropBeam from here.
        case saved(String)
        case failed(String)
    }
    struct Item: Identifiable {
        let id = UUID()
        enum Kind { case file(URL), text(String) }
        let kind: Kind
        var size: Int64 = 0
        var type: UTType?
        var thumbnail: UIImage?
        var name: String {
            switch kind {
            case .file(let url): return url.lastPathComponent
            case .text(let text): return text
            }
        }
        var isLink: Bool { if case .text(let t) = kind { return URL(string: t)?.scheme?.hasPrefix("http") == true }; return false }
    }

    @Published var phase: Phase = .preparing
    @Published var items: [Item] = []
    /// Attachments still being copied (for "Preparing 2 of 5…").
    @Published var pending: Int
    @Published var failures = 0
    @Published private(set) var recipients: [ShareGroup.Recipient] = []
    /// Presence is only shown while the app's snapshot is recent (it can't update in the background).
    @Published private(set) var presenceFresh = false
    @Published private(set) var hasSnapshot = false
    var onFinish: ((Outcome) -> Void)?

    private let providers: [NSItemProvider]
    private let jobID = UUID().uuidString
    private var stage: URL?
    private var loadTask: Task<Void, Never>?

    init(items: [NSExtensionItem]) {
        providers = items.flatMap { $0.attachments ?? [] }
        pending = providers.count
    }

    var total: Int { providers.count }
    var totalBytes: Int64 { items.reduce(0) { $0 + $1.size } }
    var fileCount: Int { items.filter { if case .file = $0.kind { return true }; return false }.count }

    func start() {
        loadRecipients()
        guard let container = ShareGroup.container else {
            phase = .failed("DropBeam isn’t set up to receive shared items. Update DropBeam and try again.")
            return
        }
        let stage = container.appendingPathComponent("ShareJobs/.incoming-\(jobID)/files", isDirectory: true)
        self.stage = stage
        do { try FileManager.default.createDirectory(at: stage, withIntermediateDirectories: true) }
        catch { phase = .failed("There isn’t room to prepare these items."); return }
        guard !providers.isEmpty else { phase = .failed("There’s nothing here DropBeam can send."); return }
        loadTask = Task { [providers] in
            // One at a time keeps memory low (the extension has a small budget) and the
            // order the user picked; copies are APFS clones, so this is fast anyway.
            for provider in providers {
                if Task.isCancelled { return }
                if let item = await Self.load(provider, into: stage) {
                    var item = item
                    if case .file(let url) = item.kind {
                        item.size = Self.size(of: url)
                        item.type = UTType(filenameExtension: url.pathExtension)
                    }
                    items.append(item)
                    let index = items.count - 1
                    if case .file(let url) = item.kind {
                        Task { if let image = await Self.thumbnail(url), index < items.count { items[index].thumbnail = image } }
                    }
                } else {
                    failures += 1
                }
                pending -= 1
            }
            if items.isEmpty { phase = .failed("DropBeam couldn’t read what you shared. If it’s in iCloud, open it once so it downloads, then try again.") }
            else if phase == .preparing { phase = .ready }
        }
    }

    private func loadRecipients() {
        guard let container = ShareGroup.container,
              let data = try? Data(contentsOf: container.appendingPathComponent("share-recipients.json")),
              let snapshot = try? JSONDecoder().decode(ShareGroup.Snapshot.self, from: data) else { return }
        hasSnapshot = true
        recipients = snapshot.recipients
        presenceFresh = Date().timeIntervalSince1970 * 1000 - snapshot.updatedMs < 5 * 60_000
    }

    func avatar(for recipient: ShareGroup.Recipient) -> UIImage? {
        guard let rel = recipient.avatar, !rel.contains(".."), let container = ShareGroup.container else { return nil }
        return UIImage(contentsOfFile: container.appendingPathComponent(rel).path)
    }

    // MARK: Actions

    func send(to recipient: ShareGroup.Recipient) { submit(kind: "friend", key: recipient.id, recipient: recipient) }
    func quickSend() { submit(kind: "quick", key: "quick", recipient: nil) }
    func chooseInApp() { submit(kind: "choose", key: "choose", recipient: nil) }

    private func submit(kind: String, key: String, recipient: ShareGroup.Recipient?) {
        guard case .ready = phase.readyish else { return }
        phase = .sending(key)
        Task {
            await loadTask?.value // Tapping early is fine: we finish copying first.
            guard !items.isEmpty, let stage, let container = ShareGroup.container else { return }
            let files = items.compactMap { item -> String? in if case .file(let url) = item.kind { return url.lastPathComponent }; return nil }
            let texts = items.compactMap { item -> String? in if case .text(let text) = item.kind { return text }; return nil }
            let job = ShareGroup.Job(id: jobID, createdMs: Date().timeIntervalSince1970 * 1000, recipient: kind,
                                     friendId: recipient?.id, friendName: recipient?.name, files: files, texts: texts)
            let incoming = stage.deletingLastPathComponent()
            let ready = container.appendingPathComponent("ShareJobs/\(jobID)", isDirectory: true)
            do {
                try JSONEncoder().encode(job).write(to: incoming.appendingPathComponent("job.json"), options: .atomic)
                // The rename is what makes the job visible to the app — never half-written.
                try FileManager.default.moveItem(at: incoming, to: ready)
            } catch {
                phase = .failed("DropBeam couldn’t save these items: \(error.localizedDescription)")
                return
            }
            self.stage = nil
            onFinish?(.open(URL(string: "dropbeam://share?job=\(jobID)")!))
        }
    }

    /// iOS wouldn't open DropBeam from the share sheet: the job is saved, so say how to
    /// finish, and leave a notification that opens the app with one tap.
    func openFailed() {
        let who: String
        if case .sending(let key) = phase, let r = recipients.first(where: { $0.id == key }) { who = r.name } else { who = "" }
        phase = .saved(who)
        let content = UNMutableNotificationContent()
        content.title = "Ready to send"
        content.body = who.isEmpty ? "Tap to finish sending with DropBeam." : "Tap to send to \(who) with DropBeam."
        UNUserNotificationCenter.current().add(UNNotificationRequest(identifier: "share-\(jobID)", content: content, trigger: nil))
    }

    func cancel() {
        loadTask?.cancel()
        if let stage { try? FileManager.default.removeItem(at: stage.deletingLastPathComponent()) }
        onFinish?(.cancelled)
    }
    func done() { onFinish?(.done) }

    // MARK: Loading attachments

    private struct LoadFailure: Error {}

    /// A web link or a text snippet stays text; everything else becomes a file copy.
    private nonisolated static func load(_ provider: NSItemProvider, into dir: URL) async -> Item? {
        let ids = provider.registeredTypeIdentifiers
        let fileTypes = ids.filter { id in
            guard let type = UTType(id) else { return false }
            // Photos also offers private/Live Photo bundles the receiver can't open.
            if id.contains(".private.") || id.contains("live-photo") { return false }
            return !type.conforms(to: .url) && !type.conforms(to: .plainText)
        }
        let isFileURL = provider.hasItemConformingToTypeIdentifier(UTType.fileURL.identifier)
        if fileTypes.isEmpty && !isFileURL {
            if provider.hasItemConformingToTypeIdentifier(UTType.url.identifier),
               let url = try? await loadObject(provider, URL.self) { return Item(kind: .text(url.absoluteString)) }
            // A text file from Files has a name; a text selection doesn't.
            if provider.suggestedName == nil, provider.hasItemConformingToTypeIdentifier(UTType.plainText.identifier),
               let text = try? await loadObject(provider, String.self),
               !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty { return Item(kind: .text(text)) }
        }
        // Best file type: the original movie/photo/document the app offers first.
        let preferred = fileTypes.first { UTType($0)?.conforms(to: .movie) == true }
            ?? fileTypes.first { UTType($0)?.conforms(to: .image) == true }
            ?? fileTypes.first ?? ids.first { UTType($0)?.conforms(to: .plainText) == true }
        if let type = preferred, let url = try? await copyFileRepresentation(provider, type: type, into: dir) {
            return Item(kind: .file(url))
        }
        // Some apps only hand over a (security-scoped) file URL.
        if isFileURL, let source = try? await loadObject(provider, URL.self), source.isFileURL,
           let url = try? copyScoped(source, into: dir, name: provider.suggestedName) {
            return Item(kind: .file(url))
        }
        // Screenshots from markup etc. may exist only as image data / a UIImage.
        if provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) {
            if let data = try? await loadData(provider, type: preferred ?? UTType.image.identifier), !data.isEmpty {
                let ext = UTType(preferred ?? "")?.preferredFilenameExtension ?? imageExtension(data)
                if let url = try? write(data, name: provider.suggestedName, ext: ext, into: dir) { return Item(kind: .file(url)) }
            }
            if let image = try? await loadImage(provider), let data = image.pngData(),
               let url = try? write(data, name: provider.suggestedName, ext: "png", into: dir) { return Item(kind: .file(url)) }
        }
        return nil
    }

    private nonisolated static func loadObject<T: _ObjectiveCBridgeable>(_ provider: NSItemProvider, _ type: T.Type) async throws -> T where T._ObjectiveCType: NSItemProviderReading {
        try await withCheckedThrowingContinuation { continuation in
            _ = provider.loadObject(ofClass: type) { value, error in
                if let value { continuation.resume(returning: value) } else { continuation.resume(throwing: error ?? LoadFailure()) }
            }
        }
    }
    private nonisolated static func loadImage(_ provider: NSItemProvider) async throws -> UIImage {
        try await withCheckedThrowingContinuation { continuation in
            _ = provider.loadObject(ofClass: UIImage.self) { value, error in
                if let image = value as? UIImage { continuation.resume(returning: image) } else { continuation.resume(throwing: error ?? LoadFailure()) }
            }
        }
    }
    private nonisolated static func loadData(_ provider: NSItemProvider, type: String) async throws -> Data {
        try await withCheckedThrowingContinuation { continuation in
            _ = provider.loadDataRepresentation(forTypeIdentifier: type) { data, error in
                if let data { continuation.resume(returning: data) } else { continuation.resume(throwing: error ?? LoadFailure()) }
            }
        }
    }

    /// The system's temporary copy only lives for the callback: clone it out right there.
    /// (Big videos never pass through memory.)
    private nonisolated static func copyFileRepresentation(_ provider: NSItemProvider, type: String, into dir: URL) async throws -> URL {
        let suggested = provider.suggestedName
        return try await withCheckedThrowingContinuation { continuation in
            _ = provider.loadFileRepresentation(forTypeIdentifier: type) { url, error in
                guard let url else { continuation.resume(throwing: error ?? LoadFailure()); return }
                do {
                    let name = fileName(suggested: suggested, fallback: url.lastPathComponent, type: type)
                    let dest = unique(name, in: dir)
                    try FileManager.default.copyItem(at: url, to: dest)
                    continuation.resume(returning: dest)
                } catch { continuation.resume(throwing: error) }
            }
        }
    }

    private nonisolated static func copyScoped(_ source: URL, into dir: URL, name: String?) throws -> URL {
        let scoped = source.startAccessingSecurityScopedResource()
        defer { if scoped { source.stopAccessingSecurityScopedResource() } }
        let dest = unique(fileName(suggested: name, fallback: source.lastPathComponent, type: nil), in: dir)
        var coordinationError: NSError?
        var copyError: Error?
        NSFileCoordinator().coordinate(readingItemAt: source, options: [], error: &coordinationError) { url in
            do { try FileManager.default.copyItem(at: url, to: dest) } catch { copyError = error }
        }
        if let error = coordinationError ?? copyError { throw error }
        return dest
    }

    private nonisolated static func write(_ data: Data, name: String?, ext: String, into dir: URL) throws -> URL {
        let base = (name?.isEmpty == false ? name! : "Image") as NSString
        let file = base.pathExtension.isEmpty ? "\(base).\(ext)" : base as String
        let dest = unique(sanitize(file), in: dir)
        try data.write(to: dest)
        return dest
    }

    /// Keep the original name ("IMG_0042.HEIC"), adding the type's extension if missing.
    private nonisolated static func fileName(suggested: String?, fallback: String, type: String?) -> String {
        var name = suggested?.isEmpty == false ? suggested! : fallback
        let ext = (fallback as NSString).pathExtension.isEmpty ? UTType(type ?? "")?.preferredFilenameExtension : (fallback as NSString).pathExtension
        if (name as NSString).pathExtension.isEmpty, let ext, !ext.isEmpty { name += "." + ext }
        return sanitize(name)
    }
    private nonisolated static func sanitize(_ name: String) -> String {
        var clean = name.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
        while clean.hasPrefix(".") { clean.removeFirst() }
        return clean.isEmpty ? "Shared item" : String(clean.prefix(200))
    }
    private nonisolated static func unique(_ name: String, in dir: URL) -> URL {
        var url = dir.appendingPathComponent(name)
        let base = (name as NSString).deletingPathExtension, ext = (name as NSString).pathExtension
        var n = 2
        while FileManager.default.fileExists(atPath: url.path) {
            url = dir.appendingPathComponent(ext.isEmpty ? "\(base) \(n)" : "\(base) \(n).\(ext)"); n += 1
        }
        return url
    }
    private nonisolated static func imageExtension(_ data: Data) -> String {
        let head = [UInt8](data.prefix(12))
        if head.starts(with: [0xFF, 0xD8]) { return "jpg" }
        if head.starts(with: [0x89, 0x50, 0x4E, 0x47]) { return "png" }
        if head.count >= 12, String(bytes: head[4..<8], encoding: .ascii) == "ftyp" { return "heic" }
        return "jpg"
    }
    private nonisolated static func size(of url: URL) -> Int64 {
        let fm = FileManager.default
        var isDir: ObjCBool = false
        guard fm.fileExists(atPath: url.path, isDirectory: &isDir) else { return 0 }
        if !isDir.boolValue { return (try? fm.attributesOfItem(atPath: url.path)[.size] as? NSNumber)?.int64Value ?? 0 }
        var total: Int64 = 0
        let e = fm.enumerator(at: url, includingPropertiesForKeys: [.fileSizeKey])
        while let file = e?.nextObject() as? URL { total += Int64((try? file.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0) }
        return total
    }
    private nonisolated static func thumbnail(_ url: URL) async -> UIImage? {
        let request = QLThumbnailGenerator.Request(fileAt: url, size: CGSize(width: 72, height: 72), scale: 3, representationTypes: .thumbnail)
        return try? await QLThumbnailGenerator.shared.generateBestRepresentation(for: request).uiImage
    }
}

private extension ShareModel.Phase {
    /// Recipients can be tapped while copies are still running.
    var readyish: ShareModel.Phase { self == .preparing ? .ready : self }
}
