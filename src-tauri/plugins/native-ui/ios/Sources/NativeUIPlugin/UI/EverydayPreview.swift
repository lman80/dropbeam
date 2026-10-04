#if targetEnvironment(simulator)
import SwiftUI
import UIKit

/// Simulator-only QA data for the everyday flows (no touch input in CI):
/// `-previewEveryday` seeds friends, transfers (an offer to accept, a received photo,
/// a send waiting for an offline friend, a Quick Send code) and shared folders.
/// `-previewScreen folders|folder|invite|advanced|recoverable` opens that screen on top.
/// `-previewLocalNetworkDenied` shows the Local Network notice.
@MainActor enum EverydayPreview {
    static func seedIfRequested() {
        let args = ProcessInfo.processInfo.arguments
        if args.contains("-previewLocalNetworkDenied") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { LanDiscovery.shared.localNetworkDenied = true }
        }
        guard args.contains("-previewEveryday") else { return }
        let b = Bridge.shared
        b.previewKeys.formUnion(["friends", "presence", "transfers", "folders"])
        b.friends = [
            Friend(id: "qa-rose", name: "Rose", endpointId: "qa-rose-eid"),
            Friend(id: "qa-tom", name: "Tom", endpointId: "qa-tom-eid"),
        ]
        b.presence = ["qa-rose": true, "qa-tom": false]
        let photo = SaveFolder.defaultFolder.appendingPathComponent("Garden.jpg")
        if !FileManager.default.fileExists(atPath: photo.path) {
            let image = UIGraphicsImageRenderer(size: CGSize(width: 600, height: 400)).jpegData(withCompressionQuality: 0.8) { ctx in
                UIColor.systemGreen.setFill(); ctx.fill(CGRect(x: 0, y: 0, width: 600, height: 400))
                UIColor.systemYellow.setFill(); ctx.cgContext.fillEllipse(in: CGRect(x: 420, y: 40, width: 120, height: 120))
            }
            try? image.write(to: photo)
        }
        b.transfers = [
            Transfer(id: "qa-offer", direction: "receive", state: "waitingForAccept", fileNames: ["Birthday video.mov"], fileCount: 1, bytesTotal: 84_000_000, friendName: "Tom"),
            Transfer(id: "qa-wait", direction: "send", state: "waitingForPeer", fileNames: ["Recipes.pdf"], fileCount: 1, bytesTotal: 1_200_000, bytesDone: 0, percent: 0, friendName: "Tom"),
            Transfer(id: "qa-code", direction: "send", state: "waitingForPeer", fileNames: ["Holiday.jpg"], fileCount: 1, bytesTotal: 3_100_000, code: "7-garden-river-lamp"),
            Transfer(id: "qa-got", direction: "receive", state: "completed", fileNames: ["Garden.jpg"], fileCount: 1, bytesTotal: 120_000, bytesDone: 120_000, percent: 100, friendName: "Rose", sharePaths: [photo.path]),
        ]
        let now = Date().timeIntervalSince1970 * 1000
        b.folders = [
            SharedFolder(id: "qa-family", pairId: "qa-family-1", name: "Family Photos", path: SaveFolder.defaultFolder.path, mode: "mirror", iAmOwner: false,
                         label: "Up to date", lastSyncedMs: now - 5 * 60_000,
                         members: [FolderMember(pairId: "qa-family-1", name: "Rose", online: true, viewer: false, friendId: "qa-rose"),
                                   FolderMember(pairId: "qa-family-2", name: "Tom", online: false, viewer: true, friendId: "qa-tom")]),
            SharedFolder(id: "qa-recipes", pairId: "qa-recipes-1", name: "Recipes", mode: "twoWay", tone: "offline", label: "Tom is offline",
                         members: [FolderMember(pairId: "qa-recipes-1", name: "Tom", friendId: "qa-tom")]),
        ]
        if let i = args.firstIndex(of: "-previewScreen"), i + 1 < args.count {
            let screen = args[i + 1]
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { present(screen) }
        }
    }
    private static func present(_ screen: String) {
        let b = Bridge.shared
        let view: AnyView
        switch screen {
        case "folders": view = AnyView(NavigationStack { SharedFoldersView() })
        case "folder": view = AnyView(NavigationStack { SharedFolderDetailView(folderID: "qa-family", initial: b.folders[0]) })
        case "invite": view = AnyView(FolderInviteSheet(invite: FolderInvite(code: "qa", folderName: "Family Photos", fromName: "Rose")))
        case "advanced": view = AnyView(NavigationStack { TransferSettingsView() })
        case "recoverable": b.selectedTab = "history"; return
        default: return
        }
        let host = UIHostingController(rootView: view.environmentObject(b).tint(.beam))
        let root = UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow?.rootViewController }.first
        var top = root
        while let next = top?.presentedViewController { top = next }
        top?.present(host, animated: false)
    }
}
#endif
