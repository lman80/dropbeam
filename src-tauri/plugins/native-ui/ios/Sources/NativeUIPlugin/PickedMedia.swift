import Foundation
import PhotosUI
import UniformTypeIdentifiers
import UIKit

/// Every copy DropBeam makes of something the user chose to send — Photos picks,
/// Files picks, a folder to send, pasted images, share-extension items — lives in
/// `Library/Application Support/dropbeam-picked/<session>/`:
///
/// - not in Documents, so it never shows up in Files › On My iPhone › DropBeam;
/// - excluded from iCloud/iTunes backup (the originals are already in Photos/Files);
/// - durable (not tmp/Caches), so a send queued for an offline friend still has its
///   files after the app was suspended or relaunched;
/// - swept: a session is removed once nothing references it any more — Send-tab copies
///   a day after their send finished, chat attachments (whose bubbles show them) after
///   30 days — and the whole area is capped at 2 GB, oldest first. Anything an
///   unfinished transfer, a queued send (fanout.json), a chat draft or the Send To
///   sheet still points at is never touched.
enum PickedMedia {
    enum Purpose: String { case send, chat }
    private static let marker = ".dropbeam-purpose-"
    static var root: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("dropbeam-picked", isDirectory: true)
    }

    /// A fresh folder for one pick (one per selection, so a Live Photo's two halves and
    /// files with the same name from one pick sit side by side).
    static func session(_ purpose: Purpose) throws -> URL {
        let fm = FileManager.default
        try ensureRoot()
        let dir = root.appendingPathComponent(UUID().uuidString, isDirectory: true)
        try fm.createDirectory(at: dir, withIntermediateDirectories: true)
        fm.createFile(atPath: dir.appendingPathComponent(marker + purpose.rawValue).path, contents: nil)
        return dir
    }

    /// Tag an existing folder (e.g. a claimed share-extension job) with its purpose.
    static func mark(_ dir: URL, _ purpose: Purpose) {
        try? ensureRoot()
        FileManager.default.createFile(atPath: dir.appendingPathComponent(marker + purpose.rawValue).path, contents: nil)
    }

    /// Create the root and keep it out of backups (applies to everything inside).
    static func ensureRoot() throws {
        var url = root
        try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? url.setResourceValues(values)
    }

    /// `name`, or `name 2`, `name 3`… so nothing in `dir` is overwritten.
    static func unique(_ name: String, in dir: URL) -> URL {
        let clean = sanitize(name)
        var url = dir.appendingPathComponent(clean)
        let base = (clean as NSString).deletingPathExtension, ext = (clean as NSString).pathExtension
        var n = 2
        while FileManager.default.fileExists(atPath: url.path) {
            url = dir.appendingPathComponent(ext.isEmpty ? "\(base) \(n)" : "\(base) \(n).\(ext)")
            n += 1
        }
        return url
    }
    static func sanitize(_ name: String) -> String {
        let trimmed = name.replacingOccurrences(of: "/", with: "-").replacingOccurrences(of: ":", with: "-")
            .trimmingCharacters(in: .whitespacesAndNewlines)
        let noDots = String(trimmed.drop { $0 == "." })
        return noDots.isEmpty ? "File" : String(noDots.prefix(180))
    }

    // MARK: Sweep

    private static var sweepTask: Task<Void, Never>?
    /// Clean up soon (coalesced). Call at launch and whenever a transfer finishes.
    @MainActor static func sweepSoon(after seconds: Double = 20) {
        sweepTask?.cancel()
        sweepTask = Task { @MainActor in
            try? await Task.sleep(for: .seconds(seconds))
            guard !Task.isCancelled else { return }
            // Never judge references before the engine's first snapshots arrived.
            let bridge = Bridge.shared
            guard bridge.settings != nil else { sweepSoon(after: 30); return }
            let keep = referencedPaths(bridge)
            await Task.detached(priority: .utility) { sweep(keeping: keep) }.value
        }
    }

    @MainActor private static func referencedPaths(_ bridge: Bridge) -> [String] {
        var paths: [String] = bridge.pendingSend + bridge.pickedToSend + bridge.chatDraftFiles
        for transfer in bridge.transfers where !["completed", "canceled"].contains(transfer.state ?? "") {
            paths += transfer.sharePaths ?? []
        }
        // Sends queued for a person's offline devices (fanout.json in the engine's folder).
        if let dir = PushRegistration.engineDir,
           let data = try? Data(contentsOf: dir.appendingPathComponent("fanout.json")),
           let records = try? JSONSerialization.jsonObject(with: data) as? [String: Any] {
            for case let record as [String: Any] in records.values { paths += record["paths"] as? [String] ?? [] }
        }
        return paths.map { URL(fileURLWithPath: $0).standardizedFileURL.path }
    }

    static let chatRetention: TimeInterval = 30 * 86_400
    static let sendRetention: TimeInterval = 86_400
    static let capBytes: Int64 = 2 * 1024 * 1024 * 1024

    /// Remove sessions nothing points at any more (see the type comment).
    static func sweep(keeping referenced: [String], now: Date = Date()) {
        let fm = FileManager.default
        let rootPath = root.standardizedFileURL.path
        guard let names = try? fm.contentsOfDirectory(atPath: rootPath) else { return }
        struct Session { let url: URL; let age: TimeInterval; let bytes: Int64; let purpose: Purpose; let protected: Bool }
        var sessions: [Session] = []
        for name in names {
            let url = URL(fileURLWithPath: rootPath).appendingPathComponent(name)
            let path = url.path
            let protected = referenced.contains { $0 == path || $0.hasPrefix(path + "/") }
            let values = try? url.resourceValues(forKeys: [.contentModificationDateKey, .creationDateKey, .isDirectoryKey])
            let date = values?.contentModificationDate ?? values?.creationDate ?? now
            let purpose: Purpose = fm.fileExists(atPath: url.appendingPathComponent(marker + "send").path) ? .send : .chat
            sessions.append(Session(url: url, age: now.timeIntervalSince(date), bytes: size(of: url), purpose: purpose, protected: protected))
        }
        var total = sessions.reduce(Int64(0)) { $0 + $1.bytes }
        var removed = 0
        for session in sessions.sorted(by: { $0.age > $1.age }) where !session.protected {
            let expired = session.age > (session.purpose == .send ? sendRetention : chatRetention)
            if expired || total > capBytes {
                if (try? fm.removeItem(at: session.url)) != nil { total -= session.bytes; removed += 1 }
            }
        }
        if removed > 0 { NSLog("DropBeam picked media: removed %d old copies", removed) }
    }

    private static func size(of url: URL) -> Int64 {
        var total: Int64 = 0
        if let size = try? url.resourceValues(forKeys: [.fileSizeKey]).fileSize { return Int64(size) }
        let walker = FileManager.default.enumerator(at: url, includingPropertiesForKeys: [.fileSizeKey])
        while let file = walker?.nextObject() as? URL { total += Int64((try? file.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0) }
        return total
    }
}

// MARK: - Photos picker (native)

/// PHPicker for photos and videos, imported into a PickedMedia session.
///
/// - At most 3 assets load at once (iCloud downloads, big videos), each with its own
///   2-minute limit; one that fails doesn't sink the rest — the caller hears
///   "N couldn't be loaded" and gets everything that did.
/// - Files keep the names they have in Photos (IMG_1234.HEIC), not provider temp names.
/// - A Live Photo travels as its still + its .MOV with the same name (#78): another
///   iPhone saves them back together as a Live Photo; a computer just gets both files.
@MainActor final class NativePhotoPicker: NSObject, PHPickerViewControllerDelegate {
    static let shared = NativePhotoPicker()
    private var continuation: CheckedContinuation<[PHPickerResult], Never>?
    struct Result { let paths: [String]; let failed: Int }

    func pick(purpose: PickedMedia.Purpose, progress: @escaping @MainActor (Int, Int) -> Void) async throws -> Result {
        guard continuation == nil else { throw Self.failure("A photo picker is already open.") }
        try await NativePresentation.waitForPickerDismissal()
        guard var presenter = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene })
            .flatMap(\.windows).first(where: \.isKeyWindow)?.rootViewController else { throw Self.failure("No active window.") }
        while let next = presenter.presentedViewController { presenter = next }
        var config = PHPickerConfiguration()
        config.selectionLimit = 0
        config.selection = .ordered
        config.filter = .any(of: [.images, .videos, .livePhotos])
        // Keep HEIC/HEVC as they are: the engine doesn't need JPEG/H.264 conversion.
        config.preferredAssetRepresentationMode = .current
        let picker = PHPickerViewController(configuration: config)
        picker.delegate = self
        picker.isModalInPresentation = true
        let results = await withCheckedContinuation { continuation in
            self.continuation = continuation
            presenter.present(picker, animated: true)
        }
        guard !results.isEmpty else { return Result(paths: [], failed: 0) }
        let dir = try PickedMedia.session(purpose)
        let providers = results.map(\.itemProvider)
        progress(0, providers.count)
        let imported = await Self.importAll(providers, into: dir, progress: progress)
        let paths = imported.compactMap { $0 }.flatMap { $0 }
        if Task.isCancelled { try? FileManager.default.removeItem(at: dir); throw CancellationError() }
        let failed = imported.filter { $0 == nil }.count
        if paths.isEmpty {
            try? FileManager.default.removeItem(at: dir)
            throw Self.failure(failed == 1
                ? "That photo couldn’t be loaded. If it’s in iCloud, check your connection or open it in Photos first."
                : "Those \(failed) items couldn’t be loaded. If they’re in iCloud, check your connection or open them in Photos first.")
        }
        return Result(paths: paths, failed: failed)
    }

    nonisolated func picker(_ picker: PHPickerViewController, didFinishPicking results: [PHPickerResult]) {
        MainActor.assumeIsolated {
            picker.dismiss(animated: true)
            let pending = continuation; continuation = nil
            pending?.resume(returning: results)
        }
    }

    /// Import in selection order with at most 3 loads in flight. nil = that item failed.
    private static func importAll(_ providers: [NSItemProvider], into dir: URL, progress: @escaping @MainActor (Int, Int) -> Void) async -> [[String]?] {
        var results = [[String]?](repeating: nil, count: providers.count)
        var done = 0
        await withTaskGroup(of: (Int, [String]?).self) { group in
            var next = 0
            func launch() {
                guard next < providers.count else { return }
                let index = next, provider = providers[index]
                next += 1
                group.addTask { (index, try? await MediaImport.load(provider, into: dir)) }
            }
            for _ in 0..<3 { launch() }
            while let (index, paths) = await group.next() {
                results[index] = paths
                done += 1
                await progress(done, providers.count)
                if !Task.isCancelled { launch() }
            }
        }
        return results
    }
    nonisolated static func failure(_ text: String) -> NSError { NSError(domain: "DropBeam.Photos", code: 1, userInfo: [NSLocalizedDescriptionKey: text]) }
}

private final class ProgressBox: @unchecked Sendable { var value: Progress? }

/// Loading one picked asset out of its NSItemProvider (off the main actor).
enum MediaImport {
    static let timeout: Duration = .seconds(120)

    static func load(_ provider: NSItemProvider, into dir: URL) async throws -> [String] {
        try await withThrowingTaskGroup(of: [String].self) { group in
            group.addTask { try await loadUntimed(provider, into: dir) }
            group.addTask { try await Task.sleep(for: timeout); throw NativePhotoPicker.failure("Timed out loading a photo.") }
            defer { group.cancelAll() }
            guard let first = try await group.next() else { throw NativePhotoPicker.failure("Nothing loaded.") }
            return first
        }
    }

    private static func loadUntimed(_ provider: NSItemProvider, into dir: URL) async throws -> [String] {
        let name = provider.suggestedName
        if provider.canLoadObject(ofClass: PHLivePhoto.self), let pair = try? await livePhoto(provider, name: name, into: dir) {
            return pair
        }
        if provider.hasItemConformingToTypeIdentifier(UTType.movie.identifier) {
            return [try await file(provider, type: UTType.movie.identifier, name: name, into: dir)]
        }
        guard provider.hasItemConformingToTypeIdentifier(UTType.image.identifier) else {
            throw NativePhotoPicker.failure("The selected item has no image or video.")
        }
        do { return [try await file(provider, type: UTType.image.identifier, name: name, into: dir)] }
        catch { return [try await imageData(provider, name: name, into: dir)] }
    }

    /// Copy the provider's file representation (valid only inside the callback).
    private static func file(_ provider: NSItemProvider, type: String, name: String?, into dir: URL) async throws -> String {
        let progress = ProgressBox()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<String, Error>) in
                progress.value = provider.loadFileRepresentation(forTypeIdentifier: type) { url, error in
                    do {
                        guard let url else { throw error ?? NativePhotoPicker.failure("Could not load the selected item.") }
                        let ext = url.pathExtension.isEmpty ? (UTType(type)?.preferredFilenameExtension ?? "dat") : url.pathExtension
                        let dest = PickedMedia.unique(Self.filename(name, fallback: url.deletingPathExtension().lastPathComponent, ext: ext), in: dir)
                        try FileManager.default.copyItem(at: url, to: dest)
                        continuation.resume(returning: dest.path)
                    } catch { continuation.resume(throwing: error) }
                }
            }
        } onCancel: { progress.value?.cancel() }
    }

    /// Last resort for images whose file representation fails (some shared albums).
    private static func imageData(_ provider: NSItemProvider, name: String?, into dir: URL) async throws -> String {
        let type = provider.registeredTypeIdentifiers.first { UTType($0)?.conforms(to: .image) == true } ?? UTType.jpeg.identifier
        return try await withCheckedThrowingContinuation { (continuation: CheckedContinuation<String, Error>) in
            _ = provider.loadDataRepresentation(forTypeIdentifier: type) { data, error in
                do {
                    guard let data, !data.isEmpty else { throw error ?? NativePhotoPicker.failure("The photo was empty.") }
                    let ext = UTType(type)?.preferredFilenameExtension ?? "jpg"
                    let dest = PickedMedia.unique(Self.filename(name, fallback: "Photo", ext: ext), in: dir)
                    try data.write(to: dest, options: .atomic)
                    continuation.resume(returning: dest.path)
                } catch { continuation.resume(throwing: error) }
            }
        }
    }

    /// A Live Photo's still + paired video, written under ONE base name.
    private static func livePhoto(_ provider: NSItemProvider, name: String?, into dir: URL) async throws -> [String] {
        let live: PHLivePhoto = try await withCheckedThrowingContinuation { continuation in
            _ = provider.loadObject(ofClass: PHLivePhoto.self) { object, error in
                if let live = object as? PHLivePhoto { continuation.resume(returning: live) }
                else { continuation.resume(throwing: error ?? NativePhotoPicker.failure("Not a Live Photo.")) }
            }
        }
        let resources = PHAssetResource.assetResources(for: live)
        guard let still = resources.first(where: { $0.type == .fullSizePhoto }) ?? resources.first(where: { $0.type == .photo }),
              let motion = resources.first(where: { $0.type == .fullSizePairedVideo }) ?? resources.first(where: { $0.type == .pairedVideo })
        else { throw NativePhotoPicker.failure("The Live Photo has no motion.") }
        let stillExt = (still.originalFilename as NSString).pathExtension.isEmpty ? "HEIC" : (still.originalFilename as NSString).pathExtension
        let motionExt = (motion.originalFilename as NSString).pathExtension.isEmpty ? "MOV" : (motion.originalFilename as NSString).pathExtension
        // Both halves share a base name nothing else in the folder uses.
        var base = PickedMedia.sanitize(name ?? (still.originalFilename as NSString).deletingPathExtension)
        var n = 2
        let original = base
        while FileManager.default.fileExists(atPath: dir.appendingPathComponent("\(base).\(stillExt)").path)
                || FileManager.default.fileExists(atPath: dir.appendingPathComponent("\(base).\(motionExt)").path) {
            base = "\(original) \(n)"; n += 1
        }
        let stillURL = dir.appendingPathComponent("\(base).\(stillExt)")
        let motionURL = dir.appendingPathComponent("\(base).\(motionExt)")
        do {
            try await write(still, to: stillURL)
            try await write(motion, to: motionURL)
        } catch {
            try? FileManager.default.removeItem(at: stillURL); try? FileManager.default.removeItem(at: motionURL)
            throw error
        }
        return [stillURL.path, motionURL.path]
    }

    private static func write(_ resource: PHAssetResource, to url: URL) async throws {
        let options = PHAssetResourceRequestOptions()
        options.isNetworkAccessAllowed = true
        try await PHAssetResourceManager.default().writeData(for: resource, toFile: url, options: options)
    }

    static func filename(_ suggested: String?, fallback: String, ext: String) -> String {
        let base = (suggested?.trimmingCharacters(in: .whitespacesAndNewlines)).flatMap { $0.isEmpty ? nil : $0 } ?? fallback
        // `suggestedName` usually has no extension; never double one up.
        let hasExt = (base as NSString).pathExtension.lowercased() == ext.lowercased()
        return hasExt ? base : "\(base).\(ext)"
    }
}
