import Foundation

struct Friend: Decodable, Identifiable {
    let id: String
    let name: String
    var endpointId: String?
    var avatar: String?
    var deviceKind: String?
    var accountPub: String?
    var autoAccept: Bool?
    var deviceOs: String?
    /// One of the user's own devices (same account) — shown as "Your Mac" etc.
    var ownDevice: Bool = false
    var ownLabel: String?
    /// Set on a friend's extra device (same account as an older record): it is
    /// shown and chatted with as part of that person, never as its own row.
    var groupedUnder: String?
    /// The name to show: "Your iPhone" for an own device, else the friend's name.
    var displayName: String { ownDevice ? (ownLabel ?? name) : name }
}
struct MyDevice: Decodable {
    var name: String?
    var endpointId: String?
    var deviceKind: String?
    var deviceOs: String?
    var accountPub: String?
    var linkedDevices: Int?
    var devices: [AccountDevice] = []
    var inAccount: Bool { !(accountPub ?? "").isEmpty }
}
/// A device in this account (the first one is this device).
struct AccountDevice: Decodable, Identifiable {
    var id: String { endpointId }
    let endpointId: String
    var friendId: String?
    var name: String
    var deviceKind: String?
    var deviceOs: String?
    var lastSyncMs: Double?
    var thisDevice: Bool
}
struct Transfer: Decodable, Identifiable {
    let id: String
    var direction: String?
    var state: String?
    var fileNames: [String]?
    var fileCount: Int?
    var bytesTotal: Double?
    var bytesDone: Double?
    var percent: Double?
    var speedBps: Double?
    var etaSeconds: Double?
    var friendName: String?
    var peer: String?
    var code: String?
    var error: String?
    var outDir: String?
    var locality: String?
    var detail: String?
    var sharePaths: [String]?
    var chatOnly: Bool?
    var chatTransfer: ChatTransferDetail?
    var active: Bool { ["starting", "waitingForPeer", "connecting", "waitingForAccept", "transferring"].contains(state ?? "") }
    var title: String { (fileCount ?? 0) > 1 ? "\(fileCount ?? 0) files" : fileNames?.first ?? "Files" }
    var status: String {
        switch state {
        case "starting": return "Getting ready"
        case "waitingForPeer": return "Waiting for a connection"
        case "waitingForAccept": return "Waiting for acceptance"
        case "connecting": return "Connecting"
        case "transferring": return direction == "send" ? "Sending" : "Receiving"
        case "completed": return direction == "send" ? "Sent" : "Received"
        case "failed": return "Transfer failed"
        case "paused": return "Paused"
        case "canceled": return "Canceled"
        default: return "Preparing"
        }
    }
}
struct ChatTransferDetail: Decodable {
    var id: String?
    var completedPaths: [String: String]?
}

struct ChatMessage: Decodable, Identifiable, Equatable {
    let id: String
    let peerId: String
    let fromMe: Bool
    let ts: Double
    var kind: String?
    var text: String?
    var files: [String]?
    var bytes: Double?
    var path: String?
    var status: String?
    var seq: Double?
    var replyTo: String?
    var replyPreview: String?
    var reactions: [ChatReaction]?
    var edited: Bool?
    var deleted: Bool?
    var gif: ChatGif?
    var fileXferId: String?
    var fileXferFailed: Bool?
    var date: Date { Date(timeIntervalSince1970: ts / 1000) }
    var preview: String { deleted == true ? "Message deleted" : text?.isEmpty == false ? (text ?? "") : files?.joined(separator: ", ") ?? "Attachment" }
}
struct ChatReaction: Decodable, Equatable {
    var emoji: String?
    var fromMe: Bool?
}
struct ChatGif: Decodable, Equatable {
    var url: String?
    var w: Double?
    var h: Double?
}
struct ChatThread: Decodable {
    let friendId: String
    let messages: [ChatMessage]
}
struct GifResult: Decodable, Identifiable {
    let id: String
    var title: String?
    var thumbUrl: String?
}
struct Settings: Decodable {
    var downloadDir: String?
    var displayName: String?
    var theme: String?
    var avatar: String?
    var showMegabits: Bool?
    var playSounds: Bool?
    var notifyOnComplete: Bool?
    var notifyOnMessage: Bool?
    var sendReadReceipts: Bool?
    var directMode: Bool?
    var preferDirectP2p: Bool?
    var requireDirect: Bool?
    var waitForDirect: Bool?
    var parallelStreams: Bool?
    var uploadLimitMbps: Double?
    var minimizeToTray: Bool?
    var launchAtLogin: Bool?
    var customRelay: String?
    var customRelayPass: String?
    var giphyApiKey: String?
    var verboseLogging: Bool?
    var showSyncPopup: Bool?
    var shareDiagnostics: Bool?
    var diagnosticsUrl: String?
    var labModeEnabled: Bool?
    var labOperatorId: String?
    var folderHistoryKeepDays: Int?
    var folderHistoryBudgetBytes: Double?
}
struct ChatOverview: Decodable {
    var peerId: String?
    var lastText: String?
    var lastTs: Double?
    var lastFromMe: Bool?
    var count: Int?
    var unread: Int?
}
struct ConnectionCheck: Decodable {
    var online: Bool?
    var path: String?
    var rttMs: Double?
    var label: String {
        guard online == true else { return "Offline · Try again later" }
        let route = path?.capitalized ?? "Online"
        return rttMs.map { "\(route) · \(Int($0)) ms" } ?? route
    }
}
struct LinkResult: Decodable {
    var endpointId: String?
    var name: String?
    var deviceKind: String?
    var deviceOs: String?
}
// Accept any JSON result for actions whose return value the UI doesn't need.
struct IgnoredResult: Decodable { init(from decoder: Decoder) throws {} }

struct HistoryEntry: Decodable, Identifiable {
    let id: String
    let direction: String
    let fileNames: [String]
    let bytesTotal: Double
    let timestampMs: Double
    var peer: String?
    var locality: String?
    var state: String?
    var outDir: String?
    var error: String?
    var date: Date { Date(timeIntervalSince1970: timestampMs / 1000) }
    var title: String { fileNames.count > 1 ? "\(fileNames.count) files" : fileNames.first ?? "Files" }
    var localPaths: [String] {
        guard state == "completed", let outDir else { return [] }
        return fileNames.filter { !$0.hasPrefix("/") && !$0.split(separator: "/").contains("..") }.map { URL(fileURLWithPath: outDir).appendingPathComponent($0).path }
    }
}
struct RecoverySummary: Decodable, Identifiable {
    var id: String { pairId }
    let pairId: String
    let folderName: String
    let bytes: Double
    let itemCount: Int
}
struct RecoveryItem: Decodable, Identifiable {
    let id: String
    let relPath: String
    let size: Double
    let timestampMs: Double
    var date: Date { Date(timeIntervalSince1970: timestampMs / 1000) }
}
struct LocationRights: Decodable, Hashable { let upload: Bool; let manage: Bool }
struct SharedLocation: Decodable, Identifiable, Hashable {
    let id: String
    let name: String
    let rights: LocationRights
    var reachable: Bool?
    var freeBytes: Double?
    var totalBytes: Double?
}
struct FriendLocations: Decodable, Identifiable {
    var id: String { friendId }
    let friendId: String
    let friendName: String
    let online: Bool
    let locations: [SharedLocation]
    var error: String?
    /// "pending" (not checked yet), "ready", "offline", "error" or "unavailable".
    var status = "pending"
    /// This friend's list request is in flight right now.
    var checking = false
    /// When this friend was last asked (ms since 1970); nil = not this session.
    var checkedAt: Double?
}
struct BrowserEntry: Decodable, Identifiable {
    var id: String { name }
    let name: String
    let isDir: Bool
    let size: Double
    let modified: Double
    var date: Date { Date(timeIntervalSince1970: modified / 1000) }
}
struct BrowserPage: Decodable {
    var entries: [BrowserEntry] = []
    var hasMore = false
    var cursor: String?
    var total: Int?
}
struct TrashResult: Decodable { let name: String; var error: String?; var trashPath: String? }
struct DownloadResult: Decodable { var transferId: String?; var skipped: [String]? }
struct FolderInvite: Decodable, Identifiable {
    var id: String { code }
    let code: String
    let folderName: String
    let fromName: String
}


// Malformed optional values must not discard a whole screen. Required identity
// keys still reject an individual record; collections skip only that record.
private struct BridgeKey: CodingKey {
    let stringValue: String
    let intValue: Int? = nil
    init(_ value: String) { stringValue = value }
    init?(stringValue: String) { self.init(stringValue) }
    init?(intValue: Int) { return nil }
}
struct LossyArray<Element: Decodable>: Decodable {
    let values: [Element]
    init(from decoder: Decoder) throws {
        var container = try decoder.unkeyedContainer()
        var result: [Element] = []
        while !container.isAtEnd {
            let item = try container.superDecoder()
            if let value = try? Element(from: item) { result.append(value) }
        }
        values = result
    }
}
extension Friend {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.id = try c.decode(String.self, forKey: BridgeKey("id"))
        self.name = (try? c.decode(String.self, forKey: BridgeKey("name"))) ?? "Unknown"
        self.endpointId = (try? c.decode(String.self, forKey: BridgeKey("endpointId")))
        self.avatar = (try? c.decode(String.self, forKey: BridgeKey("avatar")))
        self.deviceKind = (try? c.decode(String.self, forKey: BridgeKey("deviceKind")))
        self.accountPub = (try? c.decode(String.self, forKey: BridgeKey("accountPub")))
        self.autoAccept = (try? c.decode(Bool.self, forKey: BridgeKey("autoAccept")))
        self.deviceOs = (try? c.decode(String.self, forKey: BridgeKey("deviceOs")))
        self.ownDevice = (try? c.decode(Bool.self, forKey: BridgeKey("ownDevice"))) ?? false
        self.ownLabel = (try? c.decode(String.self, forKey: BridgeKey("ownLabel")))
        self.groupedUnder = (try? c.decode(String.self, forKey: BridgeKey("groupedUnder")))
    }
}
extension AccountDevice {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.endpointId = try c.decode(String.self, forKey: BridgeKey("endpointId"))
        self.friendId = (try? c.decode(String.self, forKey: BridgeKey("friendId")))
        self.name = (try? c.decode(String.self, forKey: BridgeKey("name"))) ?? "Device"
        self.deviceKind = (try? c.decode(String.self, forKey: BridgeKey("deviceKind")))
        self.deviceOs = (try? c.decode(String.self, forKey: BridgeKey("deviceOs")))
        self.lastSyncMs = (try? c.decode(Double.self, forKey: BridgeKey("lastSyncMs")))
        self.thisDevice = (try? c.decode(Bool.self, forKey: BridgeKey("thisDevice"))) ?? false
    }
}

extension MyDevice {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.name = (try? c.decode(String.self, forKey: BridgeKey("name")))
        self.endpointId = (try? c.decode(String.self, forKey: BridgeKey("endpointId")))
        self.deviceKind = (try? c.decode(String.self, forKey: BridgeKey("deviceKind")))
        self.accountPub = (try? c.decode(String.self, forKey: BridgeKey("accountPub")))
        self.linkedDevices = (try? c.decode(Int.self, forKey: BridgeKey("linkedDevices")))
        self.deviceOs = (try? c.decode(String.self, forKey: BridgeKey("deviceOs")))
        self.devices = (try? c.decode(LossyArray<AccountDevice>.self, forKey: BridgeKey("devices")))?.values ?? []
    }
}

extension Transfer {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.id = try c.decode(String.self, forKey: BridgeKey("id"))
        self.direction = (try? c.decode(String.self, forKey: BridgeKey("direction")))
        self.state = (try? c.decode(String.self, forKey: BridgeKey("state")))
        self.fileNames = (try? c.decode(LossyArray<String>.self, forKey: BridgeKey("fileNames")))?.values
        self.fileCount = (try? c.decode(Int.self, forKey: BridgeKey("fileCount")))
        self.bytesTotal = (try? c.decode(Double.self, forKey: BridgeKey("bytesTotal"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
        self.bytesDone = (try? c.decode(Double.self, forKey: BridgeKey("bytesDone"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
        self.percent = (try? c.decode(Double.self, forKey: BridgeKey("percent"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
        self.speedBps = (try? c.decode(Double.self, forKey: BridgeKey("speedBps"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
        self.etaSeconds = (try? c.decode(Double.self, forKey: BridgeKey("etaSeconds"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
        self.friendName = (try? c.decode(String.self, forKey: BridgeKey("friendName")))
        self.peer = (try? c.decode(String.self, forKey: BridgeKey("peer")))
        self.code = (try? c.decode(String.self, forKey: BridgeKey("code")))
        self.error = (try? c.decode(String.self, forKey: BridgeKey("error")))
        self.outDir = (try? c.decode(String.self, forKey: BridgeKey("outDir")))
        self.locality = (try? c.decode(String.self, forKey: BridgeKey("locality")))
        self.detail = (try? c.decode(String.self, forKey: BridgeKey("detail")))
        self.sharePaths = (try? c.decode(LossyArray<String>.self, forKey: BridgeKey("sharePaths")))?.values
        self.chatOnly = (try? c.decode(Bool.self, forKey: BridgeKey("chatOnly")))
        self.chatTransfer = (try? c.decode(ChatTransferDetail.self, forKey: BridgeKey("chatTransfer")))
    }
}

extension ChatTransferDetail {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.id = (try? c.decode(String.self, forKey: BridgeKey("id")))
        self.completedPaths = (try? c.decode([String: String].self, forKey: BridgeKey("completedPaths")))
    }
}

extension ChatMessage {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.id = try c.decode(String.self, forKey: BridgeKey("id"))
        self.peerId = try c.decode(String.self, forKey: BridgeKey("peerId"))
        self.fromMe = (try? c.decode(Bool.self, forKey: BridgeKey("fromMe"))) ?? false
        self.ts = (try? c.decode(Double.self, forKey: BridgeKey("ts"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil } ?? 0
        self.kind = (try? c.decode(String.self, forKey: BridgeKey("kind")))
        self.text = (try? c.decode(String.self, forKey: BridgeKey("text")))
        self.files = (try? c.decode(LossyArray<String>.self, forKey: BridgeKey("files")))?.values
        self.bytes = (try? c.decode(Double.self, forKey: BridgeKey("bytes"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
        self.path = (try? c.decode(String.self, forKey: BridgeKey("path")))
        self.status = (try? c.decode(String.self, forKey: BridgeKey("status")))
        self.seq = (try? c.decode(Double.self, forKey: BridgeKey("seq"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
        self.replyTo = (try? c.decode(String.self, forKey: BridgeKey("replyTo")))
        self.replyPreview = (try? c.decode(String.self, forKey: BridgeKey("replyPreview")))
        self.reactions = (try? c.decode(LossyArray<ChatReaction>.self, forKey: BridgeKey("reactions")))?.values
        self.edited = (try? c.decode(Bool.self, forKey: BridgeKey("edited")))
        self.deleted = (try? c.decode(Bool.self, forKey: BridgeKey("deleted")))
        self.gif = (try? c.decode(ChatGif.self, forKey: BridgeKey("gif")))
        self.fileXferId = (try? c.decode(String.self, forKey: BridgeKey("fileXferId")))
        self.fileXferFailed = (try? c.decode(Bool.self, forKey: BridgeKey("fileXferFailed")))
    }
}

extension ChatReaction {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.emoji = (try? c.decode(String.self, forKey: BridgeKey("emoji")))
        self.fromMe = (try? c.decode(Bool.self, forKey: BridgeKey("fromMe")))
    }
}

extension ChatGif {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.url = (try? c.decode(String.self, forKey: BridgeKey("url")))
        self.w = (try? c.decode(Double.self, forKey: BridgeKey("w"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
        self.h = (try? c.decode(Double.self, forKey: BridgeKey("h"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
    }
}

extension ChatThread {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.friendId = try c.decode(String.self, forKey: BridgeKey("friendId"))
        self.messages = (try? c.decode(LossyArray<ChatMessage>.self, forKey: BridgeKey("messages")))?.values ?? []
    }
}

extension GifResult {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.id = try c.decode(String.self, forKey: BridgeKey("id"))
        self.title = (try? c.decode(String.self, forKey: BridgeKey("title")))
        self.thumbUrl = (try? c.decode(String.self, forKey: BridgeKey("thumbUrl")))
    }
}

extension Settings {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.downloadDir = (try? c.decode(String.self, forKey: BridgeKey("downloadDir")))
        self.displayName = (try? c.decode(String.self, forKey: BridgeKey("displayName")))
        self.theme = (try? c.decode(String.self, forKey: BridgeKey("theme")))
        self.avatar = (try? c.decode(String.self, forKey: BridgeKey("avatar")))
        self.showMegabits = (try? c.decode(Bool.self, forKey: BridgeKey("showMegabits")))
        self.playSounds = (try? c.decode(Bool.self, forKey: BridgeKey("playSounds")))
        self.notifyOnComplete = (try? c.decode(Bool.self, forKey: BridgeKey("notifyOnComplete")))
        self.notifyOnMessage = (try? c.decode(Bool.self, forKey: BridgeKey("notifyOnMessage")))
        self.sendReadReceipts = (try? c.decode(Bool.self, forKey: BridgeKey("sendReadReceipts")))
        self.directMode = (try? c.decode(Bool.self, forKey: BridgeKey("directMode")))
        self.preferDirectP2p = (try? c.decode(Bool.self, forKey: BridgeKey("preferDirectP2p")))
        self.requireDirect = (try? c.decode(Bool.self, forKey: BridgeKey("requireDirect")))
        self.waitForDirect = (try? c.decode(Bool.self, forKey: BridgeKey("waitForDirect")))
        self.parallelStreams = (try? c.decode(Bool.self, forKey: BridgeKey("parallelStreams")))
        self.uploadLimitMbps = (try? c.decode(Double.self, forKey: BridgeKey("uploadLimitMbps"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
        self.minimizeToTray = (try? c.decode(Bool.self, forKey: BridgeKey("minimizeToTray")))
        self.launchAtLogin = (try? c.decode(Bool.self, forKey: BridgeKey("launchAtLogin")))
        self.customRelay = (try? c.decode(String.self, forKey: BridgeKey("customRelay")))
        self.customRelayPass = (try? c.decode(String.self, forKey: BridgeKey("customRelayPass")))
        self.giphyApiKey = (try? c.decode(String.self, forKey: BridgeKey("giphyApiKey")))
        self.verboseLogging = (try? c.decode(Bool.self, forKey: BridgeKey("verboseLogging")))
        self.showSyncPopup = (try? c.decode(Bool.self, forKey: BridgeKey("showSyncPopup")))
        self.shareDiagnostics = (try? c.decode(Bool.self, forKey: BridgeKey("shareDiagnostics")))
        self.diagnosticsUrl = (try? c.decode(String.self, forKey: BridgeKey("diagnosticsUrl")))
        self.labModeEnabled = (try? c.decode(Bool.self, forKey: BridgeKey("labModeEnabled")))
        self.labOperatorId = (try? c.decode(String.self, forKey: BridgeKey("labOperatorId")))
        self.folderHistoryKeepDays = (try? c.decode(Int.self, forKey: BridgeKey("folderHistoryKeepDays")))
        self.folderHistoryBudgetBytes = (try? c.decode(Double.self, forKey: BridgeKey("folderHistoryBudgetBytes"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
    }
}

extension ChatOverview {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.peerId = (try? c.decode(String.self, forKey: BridgeKey("peerId")))
        self.lastText = (try? c.decode(String.self, forKey: BridgeKey("lastText")))
        self.lastTs = (try? c.decode(Double.self, forKey: BridgeKey("lastTs"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
        self.lastFromMe = (try? c.decode(Bool.self, forKey: BridgeKey("lastFromMe")))
        self.count = (try? c.decode(Int.self, forKey: BridgeKey("count")))
        self.unread = (try? c.decode(Int.self, forKey: BridgeKey("unread")))
    }
}

extension ConnectionCheck {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.online = (try? c.decode(Bool.self, forKey: BridgeKey("online")))
        self.path = (try? c.decode(String.self, forKey: BridgeKey("path")))
        self.rttMs = (try? c.decode(Double.self, forKey: BridgeKey("rttMs"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
    }
}

extension LinkResult {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.endpointId = (try? c.decode(String.self, forKey: BridgeKey("endpointId")))
        self.name = (try? c.decode(String.self, forKey: BridgeKey("name")))
        self.deviceKind = (try? c.decode(String.self, forKey: BridgeKey("deviceKind")))
    }
}

extension HistoryEntry {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.id = try c.decode(String.self, forKey: BridgeKey("id"))
        self.direction = (try? c.decode(String.self, forKey: BridgeKey("direction"))) ?? ""
        self.fileNames = (try? c.decode(LossyArray<String>.self, forKey: BridgeKey("fileNames")))?.values ?? []
        self.bytesTotal = (try? c.decode(Double.self, forKey: BridgeKey("bytesTotal"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil } ?? 0
        self.timestampMs = (try? c.decode(Double.self, forKey: BridgeKey("timestampMs"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil } ?? 0
        self.peer = (try? c.decode(String.self, forKey: BridgeKey("peer")))
        self.locality = (try? c.decode(String.self, forKey: BridgeKey("locality")))
        self.state = (try? c.decode(String.self, forKey: BridgeKey("state")))
        self.outDir = (try? c.decode(String.self, forKey: BridgeKey("outDir")))
        self.error = (try? c.decode(String.self, forKey: BridgeKey("error")))
    }
}

extension RecoverySummary {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.pairId = try c.decode(String.self, forKey: BridgeKey("pairId"))
        self.folderName = (try? c.decode(String.self, forKey: BridgeKey("folderName"))) ?? ""
        self.bytes = (try? c.decode(Double.self, forKey: BridgeKey("bytes"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil } ?? 0
        self.itemCount = (try? c.decode(Int.self, forKey: BridgeKey("itemCount"))) ?? 0
    }
}

extension RecoveryItem {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.id = try c.decode(String.self, forKey: BridgeKey("id"))
        self.relPath = (try? c.decode(String.self, forKey: BridgeKey("relPath"))) ?? ""
        self.size = (try? c.decode(Double.self, forKey: BridgeKey("size"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil } ?? 0
        self.timestampMs = (try? c.decode(Double.self, forKey: BridgeKey("timestampMs"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil } ?? 0
    }
}

extension LocationRights {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.upload = (try? c.decode(Bool.self, forKey: BridgeKey("upload"))) ?? false
        self.manage = (try? c.decode(Bool.self, forKey: BridgeKey("manage"))) ?? false
    }
}

extension SharedLocation {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.id = try c.decode(String.self, forKey: BridgeKey("id"))
        self.name = (try? c.decode(String.self, forKey: BridgeKey("name"))) ?? "Unknown"
        self.rights = (try? c.decode(LocationRights.self, forKey: BridgeKey("rights"))) ?? LocationRights(upload: false, manage: false)
        self.reachable = (try? c.decode(Bool.self, forKey: BridgeKey("reachable")))
        let bytes = { (key: String) in (try? c.decode(Double.self, forKey: BridgeKey(key))).flatMap { $0.isFinite && $0 >= 0 ? $0 : nil } }
        self.freeBytes = bytes("freeBytes"); self.totalBytes = bytes("totalBytes")
    }
}

extension FriendLocations {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.friendId = try c.decode(String.self, forKey: BridgeKey("friendId"))
        self.friendName = (try? c.decode(String.self, forKey: BridgeKey("friendName"))) ?? ""
        self.online = (try? c.decode(Bool.self, forKey: BridgeKey("online"))) ?? false
        self.locations = (try? c.decode(LossyArray<SharedLocation>.self, forKey: BridgeKey("locations")))?.values ?? []
        self.error = (try? c.decode(String.self, forKey: BridgeKey("error")))
        self.status = (try? c.decode(String.self, forKey: BridgeKey("status"))) ?? (self.error == nil ? "ready" : "error")
        self.checking = (try? c.decode(Bool.self, forKey: BridgeKey("checking"))) ?? false
        self.checkedAt = (try? c.decode(Double.self, forKey: BridgeKey("checkedAt"))).flatMap { $0.isFinite && $0 > 0 ? $0 : nil }
    }
}

extension BrowserEntry {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.name = (try? c.decode(String.self, forKey: BridgeKey("name"))) ?? "Unknown"
        self.isDir = (try? c.decode(Bool.self, forKey: BridgeKey("isDir"))) ?? false
        self.size = (try? c.decode(Double.self, forKey: BridgeKey("size"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil } ?? 0
        self.modified = (try? c.decode(Double.self, forKey: BridgeKey("modified"))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil } ?? 0
    }
}

extension BrowserPage {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.entries = (try? c.decode(LossyArray<BrowserEntry>.self, forKey: BridgeKey("entries")))?.values ?? []
        self.hasMore = (try? c.decode(Bool.self, forKey: BridgeKey("hasMore"))) ?? false
        self.cursor = (try? c.decode(String.self, forKey: BridgeKey("cursor")))
        self.total = (try? c.decode(Int.self, forKey: BridgeKey("total")))
    }
}

extension TrashResult {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.name = (try? c.decode(String.self, forKey: BridgeKey("name"))) ?? "Unknown"
        self.error = (try? c.decode(String.self, forKey: BridgeKey("error")))
        self.trashPath = (try? c.decode(String.self, forKey: BridgeKey("trashPath")))
    }
}

extension DownloadResult {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.transferId = (try? c.decode(String.self, forKey: BridgeKey("transferId")))
        self.skipped = (try? c.decode(LossyArray<String>.self, forKey: BridgeKey("skipped")))?.values
    }
}

extension FolderInvite {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.code = try c.decode(String.self, forKey: BridgeKey("code"))
        self.folderName = (try? c.decode(String.self, forKey: BridgeKey("folderName"))) ?? ""
        self.fromName = (try? c.decode(String.self, forKey: BridgeKey("fromName"))) ?? ""
    }
}

// Shared Folders (snapshot key "folders", built by src/lib/nativeFolders.ts).
struct FolderMember: Decodable, Identifiable, Hashable {
    var id: String { pairId }
    let pairId: String
    var name = "Waiting to join…"
    var online = false
    var pending = false
    var viewer = false
    var friendId: String?
    var canSetRole = false
}
struct FolderSummary: Decodable, Hashable {
    var direction = "receive"
    var files = 0
    var bytes: Double = 0
    var durationMs: Double = 0
    var avgBps: Double = 0
}
struct SharedFolder: Decodable, Identifiable {
    let id: String
    let pairId: String
    var name = "Shared Folder"
    var path = ""
    /// "mirror", "twoWay", "sendOnly" or "receiveOnly".
    var mode = "mirror"
    var modeLabel = "Total sync"
    var autoDelete = false
    var iAmViewer = false
    var iAmOwner = false
    var paused = false
    var peerUnshared = false
    var state = "idle"
    /// "ok", "busy", "warn", "error" or "offline".
    var tone = "ok"
    var label = ""
    var percent: Double = 0
    var bytesDone: Double = 0
    var bytesTotal: Double = 0
    var speedBps: Double = 0
    var etaSeconds: Double?
    var currentFile: String?
    var queued = 0
    var queuedFiles: [String] = []
    var peerFiles: Int?
    var inSync = false
    var locality: String?
    var pendingInvite: String?
    var lastSyncedMs: Double?
    var summary: FolderSummary?
    var members: [FolderMember] = []
    var busy: Bool { state == "sending" || state == "receiving" }
    var mirror: Bool { mode == "mirror" }
}
struct FolderVerify: Decodable {
    var peerOnline = false
    var compared = false
    var identical = false
    var matched = 0
    var differences = 0
    var localFiles = 0
    var peerFiles = 0
}

private func finite(_ c: KeyedDecodingContainer<BridgeKey>, _ key: String) -> Double? {
    (try? c.decode(Double.self, forKey: BridgeKey(key))).flatMap { $0.isFinite && abs($0) < 9_000_000_000_000_000 ? $0 : nil }
}
extension FolderMember {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.pairId = try c.decode(String.self, forKey: BridgeKey("pairId"))
        if let name = try? c.decode(String.self, forKey: BridgeKey("name")), !name.isEmpty { self.name = name }
        self.online = (try? c.decode(Bool.self, forKey: BridgeKey("online"))) ?? false
        self.pending = (try? c.decode(Bool.self, forKey: BridgeKey("pending"))) ?? false
        self.viewer = (try? c.decode(Bool.self, forKey: BridgeKey("viewer"))) ?? false
        self.friendId = (try? c.decode(String.self, forKey: BridgeKey("friendId")))
        self.canSetRole = (try? c.decode(Bool.self, forKey: BridgeKey("canSetRole"))) ?? false
    }
}
extension FolderSummary {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.direction = (try? c.decode(String.self, forKey: BridgeKey("direction"))) ?? "receive"
        self.files = (try? c.decode(Int.self, forKey: BridgeKey("files"))) ?? 0
        self.bytes = finite(c, "bytes") ?? 0
        self.durationMs = finite(c, "durationMs") ?? 0
        self.avgBps = finite(c, "avgBps") ?? 0
    }
}
extension SharedFolder {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        let str = { (key: String) in try? c.decode(String.self, forKey: BridgeKey(key)) }
        let bool = { (key: String) in (try? c.decode(Bool.self, forKey: BridgeKey(key))) ?? false }
        self.id = try c.decode(String.self, forKey: BridgeKey("id"))
        self.pairId = try c.decode(String.self, forKey: BridgeKey("pairId"))
        if let name = str("name"), !name.isEmpty { self.name = name }
        self.path = str("path") ?? ""
        self.mode = str("mode") ?? "mirror"
        self.modeLabel = str("modeLabel") ?? (mode == "mirror" ? "Total sync" : "Two-way")
        self.autoDelete = bool("autoDelete"); self.iAmViewer = bool("iAmViewer"); self.iAmOwner = bool("iAmOwner")
        self.paused = bool("paused"); self.peerUnshared = bool("peerUnshared"); self.inSync = bool("inSync")
        self.state = str("state") ?? "idle"
        self.tone = str("tone") ?? "ok"
        self.label = str("label") ?? ""
        self.percent = min(100, max(0, finite(c, "percent") ?? 0))
        self.bytesDone = finite(c, "bytesDone") ?? 0
        self.bytesTotal = finite(c, "bytesTotal") ?? 0
        self.speedBps = finite(c, "speedBps") ?? 0
        self.etaSeconds = finite(c, "etaSeconds")
        self.currentFile = str("currentFile")
        self.queued = (try? c.decode(Int.self, forKey: BridgeKey("queued"))) ?? 0
        self.queuedFiles = (try? c.decode(LossyArray<String>.self, forKey: BridgeKey("queuedFiles")))?.values ?? []
        self.peerFiles = (try? c.decode(Int.self, forKey: BridgeKey("peerFiles")))
        self.locality = str("locality")
        self.pendingInvite = str("pendingInvite")
        self.lastSyncedMs = finite(c, "lastSyncedMs").flatMap { $0 > 0 ? $0 : nil }
        self.summary = (try? c.decode(FolderSummary.self, forKey: BridgeKey("summary")))
        self.members = (try? c.decode(LossyArray<FolderMember>.self, forKey: BridgeKey("members")))?.values ?? []
    }
}
extension FolderVerify {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        let bool = { (key: String) in (try? c.decode(Bool.self, forKey: BridgeKey(key))) ?? false }
        let int = { (key: String) in (try? c.decode(Int.self, forKey: BridgeKey(key))) ?? 0 }
        self.peerOnline = bool("peerOnline"); self.compared = bool("compared"); self.identical = bool("identical")
        self.matched = int("matched"); self.differences = int("differences"); self.localFiles = int("localFiles"); self.peerFiles = int("peerFiles")
    }
}
