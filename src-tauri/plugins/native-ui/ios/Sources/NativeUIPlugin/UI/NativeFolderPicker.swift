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

/// Import into our Files-visible sandbox. Sync operates on this durable copy,
/// never on an external provider URL whose security scope has already expired.
@MainActor final class NativeFolderPicker: NSObject, UIDocumentPickerDelegate {
    static let shared = NativeFolderPicker()
    private var continuation: CheckedContinuation<[String], Error>?
    func pick() async throws -> String? { try await select(folder: true).first }
    func pickFiles() async throws -> [String] { try await select(folder: false) }
    private func select(folder: Bool) async throws -> [String] {
        guard continuation == nil else { throw failure("A file picker is already open.") }
        try await NativePresentation.waitForPickerDismissal()
        guard var presenter = UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }).flatMap(\.windows).first(where: \.isKeyWindow)?.rootViewController else { throw failure("No active window.") }
        while let next = presenter.presentedViewController { presenter = next }
        let picker = UIDocumentPickerViewController(forOpeningContentTypes: folder ? [.folder] : [.item], asCopy: !folder)
        // Leave directoryURL unset: UIKit chooses Browse / Recents. Files import as copies.
        picker.delegate = self; picker.allowsMultipleSelection = !folder; picker.isModalInPresentation = true
        return try await withCheckedThrowingContinuation { continuation in self.continuation = continuation; presenter.present(picker, animated: true) }
    }
    func documentPickerWasCancelled(_ controller: UIDocumentPickerViewController) { finish(.success([])) }
    func documentPicker(_ controller: UIDocumentPickerViewController, didPickDocumentsAt urls: [URL]) {
        guard !urls.isEmpty else { finish(.success([])); return }
        // Coordinate provider access while its security scope is held, then keep
        // a copy in Documents for the engine after the picker is dismissed.
        Task {
            do {
                let destinations = try await Task.detached(priority: .userInitiated) {
                    var imported: [String] = []
                    do {
                        for source in urls {
                        let access = source.startAccessingSecurityScopedResource()
                        defer { if access { source.stopAccessingSecurityScopedResource() } }
                        let fm = FileManager.default
                        let documents = try fm.url(for: .documentDirectory, in: .userDomainMask, appropriateFor: nil, create: true)
                        let root = documents.appendingPathComponent("Imported Folders").appendingPathComponent(UUID().uuidString)
                        let sourcePath = source.resolvingSymlinksInPath().standardizedFileURL.path
                        let destinationPath = root.resolvingSymlinksInPath().standardizedFileURL.path
                        guard !destinationPath.hasPrefix(sourcePath + "/") else {
                            throw NSError(domain: "DropBeam.Picker", code: 1, userInfo: [NSLocalizedDescriptionKey: "Choose a subfolder instead of DropBeam’s entire storage folder."])
                        }
                        try fm.createDirectory(at: root, withIntermediateDirectories: true)
                        let destination = root.appendingPathComponent(source.lastPathComponent)
                        do {
                            var coordinationError: NSError?
                            var copyError: Error?
                            NSFileCoordinator().coordinate(readingItemAt: source, options: [], error: &coordinationError) { url in
                                do { try fm.copyItem(at: url, to: destination) }
                                catch { copyError = error }
                            }
                            if let error = coordinationError ?? copyError as NSError? { throw error }
                        }
                        catch { try? fm.removeItem(at: root); throw error }
                        imported.append(destination.path)
                        }
                        return imported
                    } catch {
                        for path in imported { try? FileManager.default.removeItem(at: URL(fileURLWithPath: path).deletingLastPathComponent()) }
                        throw error
                    }
                }.value
                finish(.success(destinations))
            } catch { finish(.failure(error)) }
        }
    }
    private func finish(_ result: Result<[String], Error>) { let pending = continuation; continuation = nil; pending?.resume(with: result) }
    private func failure(_ text: String) -> NSError { NSError(domain: "DropBeam.Picker", code: 1, userInfo: [NSLocalizedDescriptionKey: text]) }
}
