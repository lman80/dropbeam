// DropBeam Finder Sync extension — draws a badge + "From <name>" on files that
// arrived in a shared folder, reading the `com.dropbeam.from` xattr the main app
// stamps on each received file. Mirrors how LucidLink/Dropbox badge files.
//
// The set of folders to watch is advertised by the main app in
//   ~/Library/Application Support/com.dropbeam.app/finder-folders.json   (a JSON array of paths)
// so the extension stays in sync as the user adds/removes shared folders.

import Cocoa
import FinderSync

class DropBeamFinderSync: FIFinderSync {
    private let controller = FIFinderSyncController.default()
    private let badgeId = "dropbeam-from"

    override init() {
        super.init()
        reloadDirectories()
        // A small badge dot. (A custom DropBeam glyph can be dropped in later; the
        // system status image keeps it dependency-free and crisp at badge size.)
        if let img = NSImage(named: NSImage.statusAvailableName) {
            controller.setBadgeImage(img, label: "From DropBeam", forBadgeIdentifier: badgeId)
        }
    }

    // MARK: - Watched folders

    private func foldersFile() -> URL {
        FileManager.default.homeDirectoryForCurrentUser
            .appendingPathComponent("Library/Application Support/com.dropbeam.app/finder-folders.json")
    }

    private func reloadDirectories() {
        guard
            let data = try? Data(contentsOf: foldersFile()),
            let arr = (try? JSONSerialization.jsonObject(with: data)) as? [String]
        else {
            controller.directoryURLs = []
            return
        }
        controller.directoryURLs = Set(arr.map { URL(fileURLWithPath: $0) })
    }

    // MARK: - Provenance (the xattr the main app writes)

    private func senderOf(_ url: URL) -> String? {
        let path = url.path
        let name = "com.dropbeam.from"
        let len = getxattr(path, name, nil, 0, 0, 0)
        if len <= 0 { return nil }
        var buf = [UInt8](repeating: 0, count: len)
        let n = getxattr(path, name, &buf, len, 0, 0)
        if n <= 0 { return nil }
        return String(bytes: buf[0..<n], encoding: .utf8)
    }

    // MARK: - Badging

    override func requestBadgeIdentifier(for url: URL) {
        // Keep the watched set fresh as folders are added/removed.
        reloadDirectories()
        if senderOf(url) != nil {
            controller.setBadgeIdentifier(badgeId, for: url)
        } else {
            controller.setBadgeIdentifier("", for: url)
        }
    }

    // MARK: - Toolbar + context menu

    override var toolbarItemName: String { "DropBeam" }
    override var toolbarItemToolTip: String { "DropBeam — who a file came from" }
    override var toolbarItemImage: NSImage {
        NSImage(named: NSImage.networkName) ?? NSImage()
    }

    override func menu(for menuKind: FIMenuKind) -> NSMenu {
        let menu = NSMenu(title: "")
        if menuKind == .contextualMenuForItems,
            let target = FIFinderSyncController.default().selectedItemURLs()?.first,
            let from = senderOf(target)
        {
            menu.addItem(withTitle: "From \(from)", action: nil, keyEquivalent: "")
        }
        return menu
    }
}
