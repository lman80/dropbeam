import UIKit
import UniformTypeIdentifiers
import PhotosUI

@MainActor enum NativePresentation {
    static func waitForPickerDismissal() async throws {
        // Some system picker delegates return their result before the dismissal
        // animation ends. Present the Send-to sheet only after that transition.
        for _ in 0..<20 {
            guard var controller = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).flatMap(\.windows).first(where: \.isKeyWindow)?.rootViewController else { return }
            while let next = controller.presentedViewController { controller = next }
            if !(controller is UIDocumentPickerViewController) && !(controller is PHPickerViewController) && !(controller is UIAlertController) && !controller.isBeingDismissed && !controller.isBeingPresented && controller.transitionCoordinator == nil { return }
            try await Task.sleep(for: .milliseconds(50))
        }
        throw NSError(domain: "DropBeam.Picker", code: 1, userInfo: [NSLocalizedDescriptionKey: "The file picker has not closed yet. Close it and try again."])
    }
}

/// Files and folders from the document picker, always as OUR copy (the provider's
/// security scope ends with the picker):
/// - files / a folder to send → a PickedMedia session (Application Support: not shown in
///   Files, not backed up, swept once sent — and it outlives a send queued for later);
/// - a folder that becomes a Shared Folder's local copy → Documents/Shared Folders/<name>,
///   visible in Files on purpose: it's the user's synced folder.
@MainActor final class NativeFolderPicker: NSObject, UIDocumentPickerDelegate {
    static let shared = NativeFolderPicker()
    private var continuation: CheckedContinuation<[URL], Error>?
    /// Shared Folder root (sync works on this durable copy).
    func pick() async throws -> String? {
        let fm = FileManager.default
        let documents = try fm.url(for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
        let parent = documents.appendingPathComponent("Shared Folders", isDirectory: true)
        guard let source = try await select(folder: true).first else { return nil }
        let destination = PickedMedia.unique(source.lastPathComponent, in: parent)
        return try await Self.importItems([source], to: { _ in destination }, removeOnFailure: destination).first
    }
    /// Files to send / attach.
    func pickFiles(purpose: PickedMedia.Purpose = .send, onImport: (() -> Void)? = nil) async throws -> [String] {
        let urls = try await select(folder: false)
        guard !urls.isEmpty else { return [] }
        onImport?()
        let dir = try PickedMedia.session(purpose)
        do { return try await Self.importItems(urls, to: { PickedMedia.unique($0.lastPathComponent, in: dir) }, removeOnFailure: dir) }
        catch { try? FileManager.default.removeItem(at: dir); throw error }
    }
    /// A folder to SEND (Send tab, Location upload): a private copy, swept once sent.
    func pickFolderToSend(purpose: PickedMedia.Purpose = .send) async throws -> String? {
        guard let source = try await select(folder: true).first else { return nil }
        let dir = try PickedMedia.session(purpose)
        do { return try await Self.importItems([source], to: { _ in dir.appendingPathComponent(PickedMedia.sanitize(source.lastPathComponent)) }, removeOnFailure: dir).first }
        catch { try? FileManager.default.removeItem(at: dir); throw error }
    }
    private func select(folder: Bool) async throws -> [URL] {
        guard continuation == nil else { throw failure("A file picker is already open.") }
        try await NativePresentation.waitForPickerDismissal()
        guard var presenter = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).flatMap(\.windows).first(where: \.isKeyWindow)?.rootViewController else { throw failure("No active window.") }
        while let next = presenter.presentedViewController { presenter = next }
        // Folders are opened in place (copied below while the scope is held); files
        // arrive as UIKit's own temporary copies.
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: folder ? [.folder] : [.item], asCopy: !folder)
        // Leave directoryURL unset: UIKit chooses Browse / Recents.
        picker.delegate = self; picker.allowsMultipleSelection = !folder; picker.isModalInPresentation = true
        return try await withCheckedThrowingContinuation { continuation in self.continuation = continuation; presenter.present(picker, animated: true) }
    }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { finish(.success([])) }
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) { finish(.success(urls)) }
    /// Copy each picked item (coordinated, inside its security scope) off the main actor.
    private static func importItems(_ urls: [URL], to destination: @escaping @Sendable (URL) -> URL, removeOnFailure: URL) async throws -> [String] {
        try await Task.detached(priority: .userInitiated) {
            var imported: [String] = []
            let fm = FileManager.default
            for source in urls {
                let access = source.startAccessingSecurityScopedResource()
                defer { if access { source.stopAccessingSecurityScopedResource() } }
                let target = destination(source)
                let sourcePath = source.resolvingSymlinksInPath().standardizedFileURL.path
                let targetPath = target.resolvingSymlinksInPath().standardizedFileURL.path
                guard !targetPath.hasPrefix(sourcePath + "/") else {
                    throw NSError(domain: "DropBeam.Picker", code: 1, userInfo: [NSLocalizedDescriptionKey: "Choose a subfolder instead of DropBeam’s entire storage folder."])
                }
                try fm.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
                var coordinationError: NSError?
                var copyError: Error?
                NSFileCoordinator().coordinate(readingItemAt: source, options: [], error: &coordinationError) { url in
                    do { try fm.copyItem(at: url, to: target) } catch { copyError = error }
                }
                if let error = coordinationError ?? copyError as NSError? {
                    if removeOnFailure.path == target.path { try? fm.removeItem(at: target) }
                    throw error
                }
                imported.append(target.path)
            }
            return imported
        }.value
    }
    private func finish(_ result: Result<[URL], Error>) { let pending = continuation; continuation = nil; pending?.resume(with: result) }
    private func failure(_ text: String) -> NSError { NSError(domain: "DropBeam.Picker", code: 1, userInfo: [NSLocalizedDescriptionKey: text]) }
}
