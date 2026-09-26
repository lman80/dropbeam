import Foundation
import Photos
import UIKit
import UniformTypeIdentifiers
import os

private let saveLog = Logger(subsystem: "com.dropbeam.app", category: "receive-saving")

@MainActor private func topPresenter() -> UIViewController? {
    guard var top = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene })
        .flatMap(\.windows).first(where: \.isKeyWindow)?.rootViewController else { return nil }
    while let next = top.presentedViewController, !next.isBeingDismissed { top = next }
    return top
}

// MARK: - Save received photos & videos to the Photos library

/// Received photos and videos can also be added to the Photos library (Blip-style).
/// Uses ADD-ONLY access: DropBeam never reads the library. The first time media
/// arrives the user is asked once; the answer is remembered and can be changed in
/// Settings → Receiving. The received file always stays in the save folder too —
/// that folder is DropBeam's record of what arrived (History, Share, Verify Copy
/// and resume all point at it), and Photos may re-encode or dedupe what it imports.
@MainActor final class ReceivedMediaSaver: ObservableObject {
    static let shared = ReceivedMediaSaver()
    enum Choice: String { case ask, on, off }
    private static let key = "dropbeam.saveMediaToPhotos"
    @Published private(set) var choice: Choice
    private var batch: [URL] = []
    private var waiting: [URL] = []
    private var flushTask: Task<Void, Never>?
    private var asking = false
    /// Paths already handled this session (a resent identical file lands on the same path).
    private var seen = Set<String>()
    private init() {
        choice = Choice(rawValue: UserDefaults.standard.string(forKey: Self.key) ?? "") ?? .ask
    }

    static let imageExtensions: Set<String> = ["jpg", "jpeg", "heic", "heif", "png", "gif", "tif", "tiff", "webp", "bmp", "dng"]
    static let videoExtensions: Set<String> = ["mov", "mp4", "m4v", "3gp", "hevc"]
    static func kind(of url: URL) -> PHAssetResourceType? {
        let ext = url.pathExtension.lowercased()
        if imageExtensions.contains(ext) { return .photo }
        if videoExtensions.contains(ext) { return .video }
        return nil
    }

    /// Engine event `received://files`: files that just landed from a friend or a code.
    /// Chat attachments stay in the chat (like Messages) and are never auto-saved.
    func received(paths: [String], chat: Bool) {
        guard !chat, choice != .off else { return }
        let media = paths.filter { seen.insert($0).inserted }.map { URL(fileURLWithPath: $0) }.filter { Self.kind(of: $0) != nil }
        guard !media.isEmpty else { return }
        batch += media
        // A folder or multi-file send lands as several batches: gather them into one
        // prompt / one "Saved N" toast.
        flushTask?.cancel()
        flushTask = Task { [weak self] in
            try? await Task.sleep(for: .milliseconds(1200))
            guard !Task.isCancelled else { return }
            self?.flush()
        }
    }
    private func flush() {
        let urls = batch; batch = []
        switch choice {
        case .off: return
        case .on: Task { await save(urls) }
        case .ask:
            waiting += urls
            if !asking { ask() }
        }
    }
    private func ask() {
        guard let presenter = topPresenter() else { return }
        asking = true
        let alert = UIAlertController(title: "Save to Photos?",
                                      message: "Add photos and videos you receive to your photo library. They also stay in your DropBeam folder. You can change this in Settings.",
                                      preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "Don’t Save", style: .cancel) { _ in Task { @MainActor in self.answer(false) } })
        let save = UIAlertAction(title: "Save to Photos", style: .default) { _ in Task { @MainActor in self.answer(true) } }
        alert.addAction(save); alert.preferredAction = save
        presenter.present(alert, animated: true)
    }
    private func answer(_ save: Bool) {
        asking = false
        let urls = waiting; waiting = []
        set(save ? .on : .off)
        if save { Task { await self.save(urls) } }
    }
    private func set(_ value: Choice) {
        choice = value
        UserDefaults.standard.set(value.rawValue, forKey: Self.key)
    }

    /// Settings toggle. Turning it on asks iOS for add-only access right away, so the
    /// system prompt appears in context rather than when a file arrives.
    func setEnabled(_ on: Bool) async {
        guard on else { set(.off); return }
        set(.on)
        let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        if status != .authorized && status != .limited {
            set(.off)
            Bridge.shared.errorMessage = "DropBeam isn’t allowed to add to Photos. Turn it on in the Settings app → Apps → DropBeam → Photos."
        }
    }

    /// Add each file as a new asset. The file stays where it is.
    func save(_ urls: [URL]) async {
        guard !urls.isEmpty else { return }
        let status = await PHPhotoLibrary.requestAuthorization(for: .addOnly)
        guard status == .authorized || status == .limited else {
            Bridge.shared.showToast("Allow Photos access in Settings to save received photos")
            return
        }
        // Never ask for full access just for an album — only use one if it's already granted.
        let album = PHPhotoLibrary.authorizationStatus(for: .readWrite) == .authorized ? await Self.dropBeamAlbum() : nil
        var photos = 0, videos = 0, failed = 0
        for url in urls {
            guard let kind = Self.kind(of: url), FileManager.default.fileExists(atPath: url.path) else { failed += 1; continue }
            do {
                try await PHPhotoLibrary.shared().performChanges {
                    let request = PHAssetCreationRequest.forAsset()
                    let options = PHAssetResourceCreationOptions()
                    options.originalFilename = url.lastPathComponent
                    options.shouldMoveFile = false // keep the received file in the save folder
                    request.addResource(with: kind, fileURL: url, options: options)
                    if let album, let placeholder = request.placeholderForCreatedAsset {
                        PHAssetCollectionChangeRequest(for: album)?.addAssets([placeholder] as NSArray)
                    }
                }
                if kind == .video { videos += 1 } else { photos += 1 }
            } catch {
                failed += 1
                saveLog.error("save to Photos failed for \(url.lastPathComponent, privacy: .private): \(error.localizedDescription, privacy: .public)")
            }
        }
        let saved = photos + videos
        if saved > 0 {
            let what = videos == 0 ? (photos == 1 ? "photo" : "\(photos) photos")
                : photos == 0 ? (videos == 1 ? "video" : "\(videos) videos") : "\(saved) items"
            Bridge.shared.showToast(failed > 0 ? "Saved \(what) to Photos · \(failed) couldn’t be saved" : "Saved \(what) to Photos")
        } else if failed > 0 {
            Bridge.shared.showToast(failed == 1 ? "Couldn’t save that file to Photos" : "Couldn’t save \(failed) files to Photos")
        }
    }
    private static func dropBeamAlbum() async -> PHAssetCollection? {
        let options = PHFetchOptions()
        options.predicate = NSPredicate(format: "title = %@", "DropBeam")
        if let existing = PHAssetCollection.fetchAssetCollections(with: .album, subtype: .albumRegular, options: options).firstObject { return existing }
        var id: String?
        do {
            try await PHPhotoLibrary.shared().performChanges {
                id = PHAssetCollectionChangeRequest.creationRequestForAssetCollection(withTitle: "DropBeam").placeholderForCreatedAssetCollection.localIdentifier
            }
        } catch { return nil }
        guard let id else { return nil }
        return PHAssetCollection.fetchAssetCollections(withLocalIdentifiers: [id], options: nil).firstObject
    }
}

// MARK: - Save files to a folder of the user's choice

/// Where received files go. Default: the app's Documents folder, which the Files app
/// shows as "On My iPhone → DropBeam". The user can pick any folder (iCloud Drive,
/// another provider…); it is kept as a security-scoped bookmark whose scope stays
/// open while the app runs, and re-resolved at every launch. The engine always boots
/// on the default (see lib.rs) and this re-applies the chosen folder once its scope
/// is active. Partial files are staged INSIDE the destination folder, so publishing a
/// finished file is a same-folder rename even on another volume.
@MainActor final class SaveFolder: NSObject, ObservableObject, UIDocumentPickerDelegate {
    static let shared = SaveFolder()
    private static let bookmarkKey = "dropbeam.saveFolderBookmark"
    /// The chosen folder while its security scope is open; nil = the DropBeam folder.
    @Published private(set) var custom: URL?
    private var restored = false
    private var applying: String?
    private var picking: CheckedContinuation<URL?, Never>?

    static var defaultFolder: URL {
        (try? FileManager.default.url(for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true))
            ?? URL(fileURLWithPath: NSHomeDirectory()).appendingPathComponent("Documents")
    }
    var current: URL { custom ?? Self.defaultFolder }
    var displayName: String { custom?.lastPathComponent ?? "DropBeam" }
    /// "On My iPhone", "iCloud Drive" or the provider's own name.
    var place: String {
        guard let url = custom else { return "On My \(UIDevice.current.model)" }
        let path = url.standardizedFileURL.path
        if path.contains("/Mobile Documents/") { return "iCloud Drive" }
        if path.hasPrefix(Self.defaultFolder.standardizedFileURL.path) { return "On My \(UIDevice.current.model) · DropBeam" }
        return (try? url.resourceValues(forKeys: [.volumeLocalizedNameKey]).volumeLocalizedName) ?? "Another Location"
    }

    /// Re-open the saved bookmark (once per launch). A folder that is gone or
    /// unreachable falls back to the DropBeam folder and says so.
    func restoreIfNeeded() {
        guard !restored else { return }
        restored = true
        guard let data = UserDefaults.standard.data(forKey: Self.bookmarkKey) else { return }
        do {
            var stale = false
            let url = try URL(resolvingBookmarkData: data, options: [], relativeTo: nil, bookmarkDataIsStale: &stale)
            guard url.startAccessingSecurityScopedResource() else { throw Self.error("No permission to use the folder.") }
            var isDirectory: ObjCBool = false
            guard FileManager.default.fileExists(atPath: url.path, isDirectory: &isDirectory), isDirectory.boolValue else {
                url.stopAccessingSecurityScopedResource()
                throw Self.error("The folder no longer exists.")
            }
            if stale, let fresh = try? url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil) {
                UserDefaults.standard.set(fresh, forKey: Self.bookmarkKey)
            }
            custom = url
        } catch {
            saveLog.error("save folder unavailable: \(error.localizedDescription, privacy: .public)")
            UserDefaults.standard.removeObject(forKey: Self.bookmarkKey)
            custom = nil
            Bridge.shared.errorMessage = "The folder you chose for received files isn’t available anymore, so files are saved in the DropBeam folder again. You can choose another in Settings → Save Files To."
        }
    }

    /// Settings snapshot arrived: make sure the engine writes where the user chose.
    func sync(engineDir: String?) {
        restoreIfNeeded()
        let wanted = current.path
        guard let engineDir, !Self.same(engineDir, wanted), applying != wanted else { return }
        apply(wanted)
    }
    private func apply(_ path: String) {
        applying = path
        Task {
            defer { if applying == path { applying = nil } }
            do { try await Bridge.shared.updateSettings(patch: ["downloadDir": path]) }
            catch { saveLog.error("could not apply save folder: \(error.localizedDescription, privacy: .public)") }
        }
    }
    private static func same(_ a: String, _ b: String) -> Bool {
        URL(fileURLWithPath: a).standardizedFileURL.resolvingSymlinksInPath().path
            == URL(fileURLWithPath: b).standardizedFileURL.resolvingSymlinksInPath().path
    }

    /// "Choose Folder…": any folder in Files, including iCloud Drive and other providers.
    func choose() async {
        guard picking == nil, let presenter = topPresenter() else { return }
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: [.folder], asCopy: false)
        picker.delegate = self
        picker.directoryURL = current
        let picked: URL? = await withCheckedContinuation { continuation in
            picking = continuation
            presenter.present(picker, animated: true)
        }
        guard let url = picked else { return }
        guard url.startAccessingSecurityScopedResource() else {
            Bridge.shared.errorMessage = "DropBeam couldn’t get permission to use that folder. Choose another one."
            return
        }
        // The app's own DropBeam folder needs no bookmark.
        if Self.same(url.path, Self.defaultFolder.path) {
            url.stopAccessingSecurityScopedResource()
            reset()
            return
        }
        do {
            let probe = url.appendingPathComponent(".dropbeam-write-test-\(UUID().uuidString)")
            try Data().write(to: probe)
            try? FileManager.default.removeItem(at: probe)
            let bookmark = try url.bookmarkData(options: [], includingResourceValuesForKeys: nil, relativeTo: nil)
            UserDefaults.standard.set(bookmark, forKey: Self.bookmarkKey)
        } catch {
            url.stopAccessingSecurityScopedResource()
            Bridge.shared.errorMessage = "DropBeam can’t save into “\(url.lastPathComponent)”. Choose a folder you can add files to."
            return
        }
        custom?.stopAccessingSecurityScopedResource()
        custom = url
        apply(url.path)
        Bridge.shared.showToast("Received files will be saved in “\(url.lastPathComponent)”")
    }
    /// "Reset to DropBeam Folder".
    func reset() {
        custom?.stopAccessingSecurityScopedResource()
        custom = nil
        UserDefaults.standard.removeObject(forKey: Self.bookmarkKey)
        apply(Self.defaultFolder.path)
    }
    /// Opens the save folder in the Files app.
    func showInFiles() {
        var components = URLComponents()
        components.scheme = "shareddocuments"
        components.path = current.path
        if let url = components.url { UIApplication.shared.open(url) }
    }

    nonisolated func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        MainActor.assumeIsolated { finishPick(urls.first) }
    }
    nonisolated func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) {
        MainActor.assumeIsolated { finishPick(nil) }
    }
    private func finishPick(_ url: URL?) { let pending = picking; picking = nil; pending?.resume(returning: url) }
    private static func error(_ text: String) -> NSError { NSError(domain: "DropBeam.SaveFolder", code: 1, userInfo: [NSLocalizedDescriptionKey: text]) }
}
