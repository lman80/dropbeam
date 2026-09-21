import Foundation

struct Friend: Decodable, Identifiable {
    let id: String
    let name: String
    var endpointId: String?
    var avatar: String?
    var deviceKind: String?
    var accountPub: String?
    var autoAccept: Bool?
}
struct MyDevice: Decodable {
    var name: String?
    var endpointId: String?
    var deviceKind: String?
    var accountPub: String?
    var linkedDevices: Int?
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
    var preview: String { deleted == true ? "Message deleted" : text?.isEmpty == false ? text! : files?.joined(separator: ", ") ?? "Attachment" }
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
}
// Accept any JSON result for actions whose return value the UI doesn't need.
struct IgnoredResult: Decodable { init(from decoder: Decoder) throws {} }
