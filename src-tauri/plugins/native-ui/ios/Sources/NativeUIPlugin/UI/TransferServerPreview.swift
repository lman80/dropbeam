#if targetEnvironment(simulator)
import Foundation

/// Simulator-only QA data for the Transfer Server UI (the simulator has no server).
/// `-previewTransferServer` seeds friends, threads, servers and held files, and pins
/// those snapshot keys so the engine can't overwrite them. `-previewChat alex|jordan`
/// opens a seeded thread. Compiled out of every device build.
@MainActor enum TransferServerPreview {
    static func seedIfRequested() {
        let args = ProcessInfo.processInfo.arguments
        guard args.contains("-previewTransferServer") else { return }
        let b = Bridge.shared
        b.previewKeys = ["friends", "thread", "transferServers", "pendingFiles", "holdRoutes", "presence", "presenceSeen", "transfers", "chatOverview", "pushStatus"]
        let now = Date().timeIntervalSince1970 * 1000
        let min = 60_000.0, hour = 3_600_000.0
        b.friends = [
            Friend(id: "qa-alex", name: "Alex Rivera", endpointId: "qa-alex-eid"),
            Friend(id: "qa-jordan", name: "Jordan Lee", endpointId: "qa-jordan-eid"),
        ]
        b.presence = ["qa-alex": false, "qa-jordan": false]
        b.presenceSeen = ["qa-alex": now - 2 * hour, "qa-jordan": now - 5 * hour]
        b.servers = [
            UsableServer(eid: "qa-linux", name: "Linux Box", own: false, member: true, through: true, useIt: true, holdForMe: true, offer: "seen", revoked: false, paused: false, learnedMs: now - 90 * hour),
            UsableServer(eid: "qa-jordan-eid", name: "Jordan’s Mac mini", own: false, member: true, through: false, useIt: false, holdForMe: false, offer: "new", revoked: false, paused: false, learnedMs: now - hour),
            UsableServer(eid: "qa-studio", name: "Studio iMac", own: false, member: true, through: false, useIt: true, holdForMe: false, offer: "seen", revoked: false, paused: true, learnedMs: now - 200 * hour),
            UsableServer(eid: "qa-office", name: "Office PC", own: false, member: false, through: false, useIt: false, holdForMe: false, offer: "seen", revoked: true, paused: false, learnedMs: now - 400 * hour),
        ]
        b.holdRoutes = ["qa-alex": "Linux Box"]
        // `-previewPushOn` shows the registered state; default is the pre-APNs-key state.
        b.pushStatus = PushStatus(enabled: args.contains("-previewPushOn"), previews: true, servers: args.contains("-previewPushOn") ? 1 : 0)
        b.pendingFiles = [PendingFile(linkId: "qa-pending", peerId: "qa-alex", serverName: "Linux Box", bytes: 48_200_000, names: ["Site photos.zip"])]
        b.transfers = [Transfer(id: "qa-held-file", direction: "send", state: "held", fileNames: ["Floor plan.pdf"], fileCount: 1, bytesTotal: 2_400_000, bytesDone: 2_400_000, percent: 100, friendName: "Alex Rivera", chatOnly: true, heldOn: "Linux Box")]
        b.threads["qa-alex"] = [
            ChatMessage(id: "a2", peerId: "qa-alex", fromMe: false, ts: now - 9 * hour, kind: "file", files: ["Site photos.zip"], bytes: 48_200_000, fileXferId: "qa-pending", via: "Linux Box"),
            ChatMessage(id: "a1", peerId: "qa-alex", fromMe: false, ts: now - 9 * hour + min, kind: "text", text: "Landing at 6. Can you send the site photos when you get a sec?", status: "delivered", via: "Linux Box"),
            ChatMessage(id: "a3", peerId: "qa-alex", fromMe: true, ts: now - 12 * min, kind: "file", files: ["Floor plan.pdf"], bytes: 2_400_000, path: "/tmp/Floor plan.pdf", status: "held", fileXferId: "qa-held-file", heldOn: "Linux Box"),
            ChatMessage(id: "a4", peerId: "qa-alex", fromMe: true, ts: now - 11 * min, kind: "text", text: "Here’s the floor plan too. Call me when you land", status: "held", heldOn: "Linux Box"),
        ]
        b.threads["qa-jordan"] = [
            ChatMessage(id: "j1", peerId: "qa-jordan", fromMe: false, ts: now - 26 * hour, kind: "text", text: "Set up the Mac mini in the closet, it’s on all the time now", status: "delivered"),
            ChatMessage(id: "j2", peerId: "qa-jordan", fromMe: true, ts: now - 25 * hour, kind: "text", text: "Nice, that’ll help", status: "read"),
            ChatMessage(id: "j3", peerId: "qa-jordan", fromMe: true, ts: now - 20 * min, kind: "text", text: "Sending the cut tonight", status: "failed", heldOn: "Linux Box", serverNote: "full"),
        ]
        if let i = args.firstIndex(of: "-previewChat"), i + 1 < args.count {
            let id = "qa-" + args[i + 1]
            DispatchQueue.main.asyncAfter(deadline: .now() + 1.5) { b.selectedTab = "chat"; b.chatPath = [id] }
        }
    }
}
#endif
