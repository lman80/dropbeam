#if targetEnvironment(simulator)
import Foundation

/// Simulator-only QA data for accounts / friends / messages screens (one
/// simulator can't link to itself or receive a friend request).
/// `-previewAccounts` seeds a friend request, a friend who hasn't accepted yet,
/// and linked devices incl. one waiting for approval; `-openDevices` opens
/// Settings → Devices. Compiled out of every device build.
@MainActor enum AccountsPreview {
    static func seedIfRequested() {
        let args = ProcessInfo.processInfo.arguments
        let b = Bridge.shared
        if args.contains("-openDevices") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { b.openDevicesSettings() }
        }
        guard args.contains("-previewAccounts") else { return }
        b.previewKeys.formUnion(["friends", "thread", "presence", "presenceSeen", "myDevice", "friendRequests", "needsName", "chatOverview"])
        b.onboarding = false; b.needsName = false
        let now = Date().timeIntervalSince1970 * 1000
        b.friends = [
            Friend(id: "qa-priya", name: "Priya Raman", endpointId: "qa-priya-eid", deviceKind: "phone", deviceOs: "ios", awaitingAccept: true),
            Friend(id: "qa-mom", name: "Mom", endpointId: "qa-mom-eid", deviceKind: "phone", deviceOs: "ios"),
            Friend(id: "qa-mac", name: "Rose’s MacBook Air", endpointId: "qa-mac-eid", deviceKind: "laptop", accountPub: "qa-acct", deviceOs: "macos", ownDevice: true, ownLabel: "Your Mac"),
        ]
        b.presence = ["qa-priya": true, "qa-mom": false, "qa-mac": true]
        b.presenceSeen = ["qa-mom": now - 3 * 3_600_000]
        b.friendRequests = [FriendRequest(endpointId: "qa-jordan", name: "Jordan", at: now - 120_000)]
        b.myDevice = MyDevice(name: "iPhone", endpointId: "qa-me", deviceKind: "phone", deviceOs: "ios", accountPub: "qa-acct", linkedDevices: 2, devices: [
            AccountDevice(endpointId: "qa-me", friendId: nil, name: "iPhone", deviceKind: "phone", deviceOs: "ios", lastSyncMs: nil, thisDevice: true),
            AccountDevice(endpointId: "qa-mac-eid", friendId: "qa-mac", name: "Rose’s MacBook Air", deviceKind: "laptop", deviceOs: "macos", lastSyncMs: now - 60_000, thisDevice: false),
            AccountDevice(endpointId: "qa-odd", friendId: nil, name: "DESKTOP-4F2K", deviceKind: "desktop", deviceOs: "windows", lastSyncMs: nil, thisDevice: false, needsApproval: true),
        ])
        b.threads["qa-priya"] = [
            ChatMessage(id: "p1", peerId: "qa-priya", fromMe: true, ts: now - 5 * 60_000, kind: "text", text: "Hi Priya, it’s Rose! Did the photos arrive?", status: "sending"),
        ]
        if args.contains("-previewAccountsChat") {
            DispatchQueue.main.asyncAfter(deadline: .now() + 2) { b.selectedTab = "chat"; b.chatPath = ["qa-priya"] }
        }
        b.threads["qa-mom"] = [
            ChatMessage(id: "m1", peerId: "qa-mom", fromMe: false, ts: now - 4 * 3_600_000, kind: "text", text: "Landed safe", status: "delivered"),
            ChatMessage(id: "m2", peerId: "qa-mom", fromMe: true, ts: now - 3 * 3_600_000, kind: "text", text: "Great, love you", status: "delivered"),
        ]
    }
}
#endif
