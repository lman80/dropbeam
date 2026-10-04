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
    /// Other contacts this one might be (same name or photo, not linked): a hint only.
    var lookAlikeWith: [String] = []
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
    /// The account's devices minus any still waiting for approval.
    var linked: [AccountDevice] { devices.filter { !$0.needsApproval } }
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
    /// Proves the account key but no remaining device vouched for it (linked by an
    /// older build, or by a device since removed): the user must approve it (S4).
    var needsApproval: Bool = false
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
    var connDetail: ConnDetail?
    var verify: VerifyReport?
    var integrity: [FileIntegrity]?
    /// Transfer Server name: uploading to it ("transferring") or held there ("held").
    var heldOn: String?
    /// A send to a friend with several devices: where it is on each one.
    var deliveries: [Delivery]?
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
        case "held": return "Waiting on \(heldOn ?? "a Transfer Server")"
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
    /// Sender side: the Transfer Server holding this message (status "held").
    var heldOn: String?
    /// Why a server couldn't deliver: expired | lost | refused | full | paused | unreachable | needs_update.
    var serverNote: String?
    /// Receiver side: arrived through this Transfer Server.
    var via: String?
    /// Sender side: a file sent to a friend's several devices — where it is on each.
    var deliveries: [Delivery]?
    /// A link preview the sender's device fetched (#47); nothing is fetched here.
    var linkPreview: ChatLinkPreview?
    var date: Date { Date(timeIntervalSince1970: ts / 1000) }
    var preview: String { deleted == true ? "Message deleted" : text?.isEmpty == false ? (text ?? "") : files?.joined(separator: ", ") ?? "Attachment" }
}
struct ChatReaction: Decodable, Equatable {
    var emoji: String?
    var fromMe: Bool?
}
/// #47: title / site / a small JPEG (data: URL) that came with the message.
struct ChatLinkPreview: Decodable, Equatable {
    var url: String
    var title: String?
    var description: String?
    var siteName: String?
    var image: String?
    var imageW: Double?
    var imageH: Double?
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
    var linkPreviews: Bool?
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
    /// The result in plain words (no latency numbers or route jargon).
    var plainLabel: String {
        guard online == true else { return "Not reachable" }
        switch path {
        case "local": return "Same network"
        case "direct": return "Direct connection"
        case "relay", "internet": return "Through a relay"
        default: return "Connected"
        }
    }
}
struct LinkResult: Decodable {
    var endpointId: String?
    var name: String?
    var deviceKind: String?
    var deviceOs: String?
}
/// `linkDevicePrepare`: whose device-link code was scanned, and the safety code
/// both devices show before anything is linked (S1).
struct LinkPreviewInfo: Decodable {
    var name: String
    var safety: String
    /// "give": that device gets this account; "take": this device joins theirs.
    var direction: String
    /// False when the other device is too old to show the safety code.
    var peerShowsCode: Bool
}
/// `link://confirm`: another device scanned this one's code and waits for a yes.
struct LinkConfirmRequest: Equatable {
    var endpointId: String
    var name: String
    var safety: String
    var joining: Bool
    init?(_ payload: Any?) {
        guard let p = payload as? [String: Any], let eid = p["endpointId"] as? String, !eid.isEmpty,
              let safety = p["safety"] as? String, !safety.isEmpty else { return nil }
        endpointId = eid
        let n = (p["name"] as? String)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        name = n.isEmpty ? "Your other device" : n
        self.safety = safety
        joining = p["joining"] as? Bool ?? false
    }
}
/// Someone new who introduced themselves: a request, not yet a friend (S2).
struct FriendRequest: Decodable, Identifiable, Equatable {
    var endpointId: String
    var name: String
    /// When they asked (ms since 1970).
    var at: Double?
    var id: String { endpointId }
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
    var integrity: [FileIntegrity]?
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
/// A person the user blocked (their devices folded together), for Settings → Blocked.
struct BlockedPerson: Decodable, Identifiable, Equatable {
    let id: String
    let name: String
    var at: Double = 0
    var endpointIds: [String] = []
}
/// A report reason offered in the Report sheet (the list lives in src/lib/report.ts).
struct ReportReason: Decodable, Identifiable, Hashable {
    let id: String
    let label: String
}
/// A ready-to-send report email built by the bridge (src/lib/report.ts).
struct ReportMail: Decodable {
    let url: String
    let to: String
    let subject: String
    let body: String
}

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
        self.avatar = (try? c.decode(String.self, forKey: BridgeKey("avatar"))).flatMap { $0.isEmpty ? nil : $0 }
        self.deviceKind = (try? c.decode(String.self, forKey: BridgeKey("deviceKind")))
        self.accountPub = (try? c.decode(String.self, forKey: BridgeKey("accountPub")))
        self.autoAccept = (try? c.decode(Bool.self, forKey: BridgeKey("autoAccept")))
        self.deviceOs = (try? c.decode(String.self, forKey: BridgeKey("deviceOs")))
        self.ownDevice = (try? c.decode(Bool.self, forKey: BridgeKey("ownDevice"))) ?? false
        self.ownLabel = (try? c.decode(String.self, forKey: BridgeKey("ownLabel")))
        self.groupedUnder = (try? c.decode(String.self, forKey: BridgeKey("groupedUnder")))
        self.lookAlikeWith = (try? c.decode([String].self, forKey: BridgeKey("lookAlikeWith"))) ?? []
    }
}
extension BlockedPerson {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.id = try c.decode(String.self, forKey: BridgeKey("id"))
        self.name = (try? c.decode(String.self, forKey: BridgeKey("name"))).flatMap { $0.isEmpty ? nil : $0 } ?? "Unknown"
        self.at = (try? c.decode(Double.self, forKey: BridgeKey("at"))) ?? 0
        self.endpointIds = (try? c.decode(LossyArray<String>.self, forKey: BridgeKey("endpointIds")))?.values ?? []
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
        self.needsApproval = (try? c.decode(Bool.self, forKey: BridgeKey("needsApproval")))
            ?? (try? c.decode(Bool.self, forKey: BridgeKey("needs_approval"))) ?? false
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
        self.connDetail = (try? c.decode(ConnDetail.self, forKey: BridgeKey("connDetail")))
        self.verify = (try? c.decode(VerifyReport.self, forKey: BridgeKey("verify")))
        self.integrity = (try? c.decode(LossyArray<FileIntegrity>.self, forKey: BridgeKey("integrity")))?.values
        self.heldOn = (try? c.decode(String.self, forKey: BridgeKey("heldOn"))).flatMap { $0.isEmpty ? nil : $0 }
        self.deliveries = (try? c.decode(LossyArray<Delivery>.self, forKey: BridgeKey("deliveries")))?.values
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
        self.heldOn = (try? c.decode(String.self, forKey: BridgeKey("heldOn"))).flatMap { $0.isEmpty ? nil : $0 }
        self.serverNote = (try? c.decode(String.self, forKey: BridgeKey("serverNote"))).flatMap { $0.isEmpty ? nil : $0 }
        self.via = (try? c.decode(String.self, forKey: BridgeKey("via"))).flatMap { $0.isEmpty ? nil : $0 }
        self.deliveries = (try? c.decode(LossyArray<Delivery>.self, forKey: BridgeKey("deliveries")))?.values
        self.linkPreview = (try? c.decode(ChatLinkPreview.self, forKey: BridgeKey("linkPreview")))
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
        self.avatar = (try? c.decode(String.self, forKey: BridgeKey("avatar"))).flatMap { $0.isEmpty ? nil : $0 }
        self.showMegabits = (try? c.decode(Bool.self, forKey: BridgeKey("showMegabits")))
        self.playSounds = (try? c.decode(Bool.self, forKey: BridgeKey("playSounds")))
        self.notifyOnComplete = (try? c.decode(Bool.self, forKey: BridgeKey("notifyOnComplete")))
        self.notifyOnMessage = (try? c.decode(Bool.self, forKey: BridgeKey("notifyOnMessage")))
        self.sendReadReceipts = (try? c.decode(Bool.self, forKey: BridgeKey("sendReadReceipts")))
        self.linkPreviews = (try? c.decode(Bool.self, forKey: BridgeKey("linkPreviews")))
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

extension FriendRequest {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.endpointId = try c.decode(String.self, forKey: BridgeKey("endpointId"))
        let name = ((try? c.decode(String.self, forKey: BridgeKey("name"))) ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        self.name = name.isEmpty ? "Someone" : name
        self.at = (try? c.decode(Double.self, forKey: BridgeKey("at"))).flatMap { $0.isFinite && $0 > 0 ? $0 : nil }
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
        self.integrity = (try? c.decode(LossyArray<FileIntegrity>.self, forKey: BridgeKey("integrity")))?.values
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

/// Live path of a transfer ("local" | "direct" | "relay" | "connecting").
struct ConnDetail: Decodable, Equatable {
    var path: String?
    var rttMs: Double?
    var upgrading: Bool
    var relay: String?
}
/// "Verify copy": a full re-hash of every file here and on the peer.
struct VerifyReport: Decodable, Equatable {
    var state: String
    var checked: Int
    var total: Int
    var mismatched: [String]
    var missing: [String]
    var error: String?
}
/// Per-file end-to-end integrity (hash checked on both ends).
struct FileIntegrity: Decodable, Identifiable, Equatable {
    var id: String { "\(index ?? -1)|\(name)" }
    var index: Int?
    var name: String
    var size: Double?
    var algorithm: String?
    var digest: String?
    var verified: Bool
}
/// What the Send screen's code field did with a code (see nativeBridge openAnyCode).
struct OpenCodeResult: Decodable {
    var kind: String
    var code: String?
    var name: String?
    /// Set when a friend was added: drives "Waiting for Alex…" → "Connected".
    var friendId: String?
}
/// `describeCode`: what a pasted/scanned/linked text holds (nothing is done yet).
struct CodeDescription: Decodable {
    var kind: String
    var code: String
    var name: String?
}
extension ConnDetail {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.path = (try? c.decode(String.self, forKey: BridgeKey("path")))
        self.rttMs = (try? c.decode(Double.self, forKey: BridgeKey("rttMs"))).flatMap { $0.isFinite && $0 >= 0 && $0 < 1e9 ? $0 : nil }
        self.upgrading = (try? c.decode(Bool.self, forKey: BridgeKey("upgrading"))) ?? false
        self.relay = (try? c.decode(String.self, forKey: BridgeKey("relay")))
    }
}
extension VerifyReport {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.state = (try? c.decode(String.self, forKey: BridgeKey("state"))) ?? "failed"
        self.checked = (try? c.decode(Int.self, forKey: BridgeKey("checked"))) ?? 0
        self.total = (try? c.decode(Int.self, forKey: BridgeKey("total"))) ?? 0
        self.mismatched = (try? c.decode(LossyArray<String>.self, forKey: BridgeKey("mismatched")))?.values ?? []
        self.missing = (try? c.decode(LossyArray<String>.self, forKey: BridgeKey("missing")))?.values ?? []
        self.error = (try? c.decode(String.self, forKey: BridgeKey("error")))
    }
}
extension FileIntegrity {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.name = try c.decode(String.self, forKey: BridgeKey("name"))
        self.index = (try? c.decode(Int.self, forKey: BridgeKey("index")))
        self.size = (try? c.decode(Double.self, forKey: BridgeKey("size"))).flatMap { $0.isFinite && $0 >= 0 && $0 < 9e18 ? $0 : nil }
        self.algorithm = (try? c.decode(String.self, forKey: BridgeKey("algorithm")))
        self.digest = (try? c.decode(String.self, forKey: BridgeKey("digest")))
        self.verified = (try? c.decode(Bool.self, forKey: BridgeKey("verified"))) ?? false
    }
}
extension OpenCodeResult {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.kind = (try? c.decode(String.self, forKey: BridgeKey("kind"))) ?? "unknown"
        self.code = (try? c.decode(String.self, forKey: BridgeKey("code")))
        self.name = (try? c.decode(String.self, forKey: BridgeKey("name")))
    }
}
/// Plain-language transfer route (mirrors the desktop path badge).
extension Transfer {
    /// "Local" / "Direct" / "Relay" (+ RTT) once connected; nil before a route exists.
    var routeLabel: String? {
        let path = connDetail?.path ?? (locality == "internet" ? "relay" : locality)
        let name: String
        switch path {
        case "local": name = "Local"
        case "direct": name = "Direct"
        case "relay", "internet": name = connDetail?.upgrading == true ? "Relay · going direct" : "Relay"
        default: return nil
        }
        if let rtt = connDetail?.rttMs { return "\(name) · \(Int(rtt.rounded())) ms" }
        return name
    }
    /// Every file's end-to-end check passed.
    var integrityVerified: Bool { !(integrity ?? []).isEmpty && (integrity ?? []).allSatisfy(\.verified) }
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

/// A Transfer Server this device may use (a friend's always-on computer, or the user's own).
struct UsableServer: Decodable, Identifiable, Equatable {
    var id: String { eid }
    let eid: String
    var name: String
    var own: Bool
    var member: Bool
    var through: Bool
    var useIt: Bool
    var holdForMe: Bool
    /// "new" → show the one-time offer card; "seen" | "dismissed" | "".
    var offer: String
    var revoked: Bool
    var paused: Bool
    var learnedMs: Double
    /// The server says it's ours (this device is one of its owner's devices).
    var owner: Bool = false
    /// We let our friends use it.
    var shareFriends: Bool = false
    var access: String = ""
    /// Friends' devices that told us about it (it's their own server).
    var via: [String] = []
    var viaPeer: String?
    var viaName: String?
    /// The short line under the server's name (same wording as desktop).
    var status: String {
        if revoked { return "No longer available" }
        if paused { return "Paused by its owner" }
        if own { return "Yours · holds your messages and sends for you" }
        if owner && shareFriends { return "Yours · shared with your friends" }
        if owner && !useIt { return "Set up as yours · not in use yet" }
        if let viaName, !useIt, !holdForMe { return "\(viaName)’s · not in use" }
        if useIt && holdForMe { return "Holds your messages and sends for you" }
        if useIt { return "Sends for you when friends are offline" }
        return "Not in use"
    }
}
extension UsableServer {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.eid = try c.decode(String.self, forKey: BridgeKey("eid"))
        self.name = (try? c.decode(String.self, forKey: BridgeKey("name"))).flatMap { $0.isEmpty ? nil : $0 } ?? "Transfer Server"
        self.own = (try? c.decode(Bool.self, forKey: BridgeKey("own"))) ?? false
        self.member = (try? c.decode(Bool.self, forKey: BridgeKey("member"))) ?? false
        self.through = (try? c.decode(Bool.self, forKey: BridgeKey("through"))) ?? false
        self.useIt = (try? c.decode(Bool.self, forKey: BridgeKey("useIt"))) ?? false
        self.holdForMe = (try? c.decode(Bool.self, forKey: BridgeKey("holdForMe"))) ?? false
        self.offer = (try? c.decode(String.self, forKey: BridgeKey("offer"))) ?? ""
        self.revoked = (try? c.decode(Bool.self, forKey: BridgeKey("revoked"))) ?? false
        self.paused = (try? c.decode(Bool.self, forKey: BridgeKey("paused"))) ?? false
        self.learnedMs = (try? c.decode(Double.self, forKey: BridgeKey("learnedMs"))) ?? 0
        self.owner = (try? c.decode(Bool.self, forKey: BridgeKey("owner"))) ?? false
        self.shareFriends = (try? c.decode(Bool.self, forKey: BridgeKey("shareFriends"))) ?? false
        self.access = (try? c.decode(String.self, forKey: BridgeKey("access"))) ?? ""
        self.via = (try? c.decode([String].self, forKey: BridgeKey("via"))) ?? []
        self.viaPeer = try? c.decode(String.self, forKey: BridgeKey("viaPeer"))
        self.viaName = (try? c.decode(String.self, forKey: BridgeKey("viaName"))).flatMap { $0.isEmpty ? nil : $0 }
    }
}
/// A file a friend sent through a Transfer Server that waits for your OK.
struct PendingFile: Decodable, Identifiable, Equatable {
    var id: String { linkId }
    let linkId: String
    var peerId: String
    var serverName: String
    var bytes: Double
    var names: [String]
}
extension PendingFile {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.linkId = try c.decode(String.self, forKey: BridgeKey("linkId"))
        self.peerId = (try? c.decode(String.self, forKey: BridgeKey("peerId"))) ?? ""
        self.serverName = (try? c.decode(String.self, forKey: BridgeKey("serverName"))).flatMap { $0.isEmpty ? nil : $0 } ?? "the Transfer Server"
        self.bytes = (try? c.decode(Double.self, forKey: BridgeKey("bytes"))).flatMap { $0.isFinite ? $0 : nil } ?? 0
        self.names = (try? c.decode(LossyArray<String>.self, forKey: BridgeKey("names")))?.values ?? []
    }
}
/// Transfer Server copy shared by chat and settings (mirrors src/lib/transferServer.ts).
enum ServerCopy {
    /// The short line under an undelivered message for a server note.
    static func note(_ note: String?, friend: String, server: String?) -> String? {
        let box = server?.isEmpty == false ? server! : "the Transfer Server"
        let cap = box.prefix(1).uppercased() + box.dropFirst()
        switch note {
        case "expired": return "\(cap) couldn’t deliver it in time · will send when \(friend) is online"
        case "lost", "refused": return "\(cap) couldn’t deliver it · will send when \(friend) is online"
        case "full": return "\(cap) is full · will send when \(friend) is online"
        case "paused": return "\(cap) is paused · will send when \(friend) is online"
        case "unreachable": return "Couldn’t reach \(box) · will keep trying"
        case "needs_update": return "\(friend) needs to update DropBeam to get messages while offline"
        default: return nil
        }
    }
    static func held(server: String?, friend: String) -> String {
        "Held on \(server ?? "your Transfer Server") — reaches \(friend) when they’re online"
    }
    static func firstName(_ name: String) -> String { name.split(separator: " ").first.map(String.init) ?? name }
}
/// Whether Transfer Servers can wake this iPhone (push_status).
struct PushStatus: Decodable, Equatable {
    var enabled = false
    var previews = true
    var servers = 0
}
extension PushStatus {
    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: BridgeKey.self)
        self.enabled = (try? c.decode(Bool.self, forKey: BridgeKey("enabled"))) ?? false
        self.previews = (try? c.decode(Bool.self, forKey: BridgeKey("previews"))) ?? true
        self.servers = (try? c.decode(Int.self, forKey: BridgeKey("servers"))) ?? 0
    }
}
