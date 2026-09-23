import Foundation
import Combine
import WebKit
import Network
import UIKit
import UserNotifications

@MainActor
final class Bridge: ObservableObject {
    static let shared = Bridge()
    @Published var preparingMedia: String?
    @Published var networkAvailable: Bool?
    private let networkMonitor = NWPathMonitor()
    private var mediaTask: Task<[String], Error>?
    private var mediaToken: UUID?
    private init() {
        networkMonitor.pathUpdateHandler = { [weak self] path in
            let available = path.status == .satisfied
            Task { @MainActor in self?.networkAvailable = available }
        }
        networkMonitor.start(queue: DispatchQueue(label: "dropbeam.native.network"))
        // The notification plugin can zero the icon badge; re-assert ours whenever
        // the app comes forward or leaves, so the Home Screen count stays right.
        for name in [UIApplication.didBecomeActiveNotification, UIApplication.willResignActiveNotification] {
            NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { _ in
                Task { @MainActor in Bridge.shared.updateAppBadge(force: true) }
            }
        }
    }
    @Published var history: [HistoryEntry] = []
    @Published var locations: [FriendLocations] = []
    @Published var needsName = false
    @Published var pendingSend: [String] = []
    @Published var folderInvites: [FolderInvite] = []
    @Published var toast: String?
    @Published var friends: [Friend] = []
    @Published var myDevice: MyDevice?
    @Published var transfers: [Transfer] = []
    @Published var settings: Settings?
    @Published var chatOverview: [ChatOverview] = []
    @Published var chatUnread: [String: Int] = [:] { didSet { updateAppBadge() } }
    @Published var folders: [SharedFolder] = []
    @Published var chatTyping: [String: Bool] = [:]
    @Published var threads: [String: [ChatMessage]] = [:]
    @Published var chatDraftFiles: [String] = []
    @Published var chatPath: [String] = []
    @Published var presence: [String: Bool] = [:]
    @Published var selectedTab = "send"
    @Published var errorMessage: String?
    weak var webview: WKWebView?
    private var nextID = 0
    private struct Pending {
        let continuation: CheckedContinuation<Data, Error>
        let name: String
        let timeout: DispatchWorkItem
    }
    private var pending: [Int: Pending] = [:]
    private let decoder = JSONDecoder()
    var unread: Int { chatUnread.values.reduce(0) { min(9999, $0 + min(9999, max(0, $1))) } }
    var sendTransfers: [Transfer] { transfers.filter { $0.chatOnly != true } }
    private var appliedBadge: Int?
    /// Unread chats on the app icon. Badge permission is requested together with
    /// alerts/sounds at first launch (notification plugin: [.badge, .alert, .sound]);
    /// without it iOS simply ignores the count.
    func updateAppBadge(force: Bool = false) {
        let count = unread
        guard force || count != appliedBadge else { return }
        appliedBadge = count
        UNUserNotificationCenter.current().setBadgeCount(count) { _ in }
    }

    func call<T: Decodable>(_ name: String, _ args: [String: Any] = [:]) async throws -> T {
        guard let webview else { throw failure("The app bridge is not ready.") }
        nextID += 1
        let id = nextID
        // Serialize all arguments, including the action name, as JSON; never
        // interpolate user text or paths into executable JavaScript.
        let encoded = try JSONSerialization.data(withJSONObject: [id, name, args])
        let json = String(decoding: encoded, as: UTF8.self)
        let data: Data = try await withTaskCancellationHandler {
          try Task.checkCancellation()
          return try await withCheckedThrowingContinuation { continuation in
            let timeout = DispatchWorkItem { [weak self] in
                self?.finish(id, .failure(self?.failure("This action timed out. Check its status before trying again.") ?? NSError(domain: "NativeUI", code: 1)))
            }
            pending[id] = Pending(continuation: continuation, name: name, timeout: timeout)
            // Picker calls include the time the user spends browsing Photos/Files; never cut them short.
            let seconds: Double = ["pickFiles", "sendChatFiles"].contains(name) ? 1800 : ["setAvatar", "browserUpload", "acceptFolderInvite"].contains(name) ? 1800 : name.hasPrefix("browser") || ["locationsList", "locationsRefresh"].contains(name) ? 180 : 30
            DispatchQueue.main.asyncAfter(deadline: .now() + seconds, execute: timeout)
            webview.evaluateJavaScript("window.__dbBridge.call(...\(json)); void 0") { [weak self] _, error in
                if let error { self?.finish(id, .failure(error)) }
            }
          }
        } onCancel: {
            Task { @MainActor in self.finish(id, .failure(CancellationError())) }
        }
        try Task.checkCancellation()
        return try decoder.decode(T.self, from: data)
    }
    private func finish(_ id: Int, _ result: Result<Data, Error>) {
        guard let item = pending.removeValue(forKey: id) else { return }
        item.timeout.cancel()
        // Drop the touch-blocking preparation overlay in the reply itself,
        // before decoding/staging or waiting for the calling task to resume.
        if item.name == "pickFiles" { preparingMedia = nil }
        item.continuation.resume(with: result)
    }
    func reply(id: Int, ok: Bool, value: Any) {
        do {
            if ok { finish(id, .success(try JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed]))) }
            else { finish(id, .failure(failure(value as? String ?? "The action failed."))) }
        } catch { finish(id, .failure(error)) }
    }
    func update(key: String, value: Any) throws {
        let data = try JSONSerialization.data(withJSONObject: value, options: [.fragmentsAllowed])
        switch key {
        case "history": history = try decoder.decode(LossyArray<HistoryEntry>.self, from: data).values
        case "locations": locations = try decoder.decode(LossyArray<FriendLocations>.self, from: data).values
        case "needsName": needsName = try decoder.decode(Bool.self, from: data)
        case "pendingSend": pendingSend = try decoder.decode(LossyArray<String>.self, from: data).values
        case "friends": friends = try decoder.decode(LossyArray<Friend>.self, from: data).values
        case "myDevice": myDevice = try decoder.decode(MyDevice?.self, from: data)
        case "transfers": transfers = try decoder.decode(LossyArray<Transfer>.self, from: data).values
        case "settings": settings = try decoder.decode(Settings?.self, from: data)
        case "chatOverview": chatOverview = try decoder.decode(LossyArray<ChatOverview>.self, from: data).values
        case "chatUnread": chatUnread = try decoder.decode([String: Int].self, from: data)
        case "chatTyping": chatTyping = try decoder.decode([String: Bool].self, from: data)
        case "chatDraftFiles": chatDraftFiles = try decoder.decode(LossyArray<String>.self, from: data).values
        case "thread":
            if let thread = try decoder.decode(ChatThread?.self, from: data) { threads[thread.friendId] = thread.messages }
        case "presence": presence = try decoder.decode([String: Bool].self, from: data)
        case "folders": folders = try decoder.decode(LossyArray<SharedFolder>.self, from: data).values
        default: break // Forward-compatible snapshots.
        }
    }
    func event(name: String, payload: Any) {
        let object = payload as? [String: Any] ?? [:]
        if name == "view", let tab = object["name"] as? String,
           ["send", "friends", "chat", "history", "settings"].contains(tab) { selectedTab = tab }
        if name == "error" { errorMessage = object["message"] as? String }
        if name == "chatOpen" {
            if let id = object["friendId"] as? String { chatPath = [id]; selectedTab = "chat" }
            else { chatPath = [] }
        }
        if name == "folder-invite://incoming", let data = try? JSONSerialization.data(withJSONObject: payload),
           let invite = try? decoder.decode(FolderInvite.self, from: data), !folderInvites.contains(where: { $0.code == invite.code }) { folderInvites.append(invite) }
        // Tauri events remain available to subsequent native chat/presence views;
        // authoritative published data always comes from the store snapshots.
        NotificationCenter.default.post(name: Notification.Name("DropBeam.\(name)"), object: nil, userInfo: ["payload": payload])
    }
    func perform(_ action: @escaping @MainActor () async throws -> Void) {
        Haptics.tap()
        Task {
            do { try await action() }
            catch is CancellationError {}
            catch {
                // A picker can report failure before its dismissal animation ends.
                let reason = error.localizedDescription
                try? await NativePresentation.waitForPickerDismissal()
                errorMessage = reason
            }
        }
    }
    private func failure(_ message: String) -> NSError { NSError(domain: "DropBeam.NativeUI", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
    func action(_ name: String, _ args: [String: Any] = [:]) async throws {
        let _: IgnoredResult = try await call(name, args)
    }
    func pickAndSend(source: String, friendId: String? = nil) async throws {
        let paths = try await pickFiles(source: source)
        guard !paths.isEmpty else { return }
        try await NativePresentation.waitForPickerDismissal()
        if let friendId { try await sendToFriend(friendId: friendId, paths: paths) }
        else { pendingSend = paths }
    }
    func showToast(_ message: String) {
        toast = message
        Task { try? await Task.sleep(for: .seconds(5)); if toast == message { toast = nil } }
    }
    func sendToFriend(friendId: String, paths: [String]) async throws { try await action("sendToFriend", ["friendId": friendId, "paths": paths]) }
    func receiveWithCode(code: String) async throws { try await action("receiveWithCode", ["code": code]) }
    func cancelTransfer(id: String) async throws { try await action("cancelTransfer", ["id": id]) }
    func retryTransfer(id: String) async throws { try await action("retryTransfer", ["id": id]) }
    func openChat(friendId: String) async throws { try await action("openChat", ["friendId": friendId]); selectedTab = "chat" }
    func chatThread(friendId: String) async throws {
        // Snapshot pushes are authoritative: an older reply must not overwrite a
        // newer reaction/read-receipt arriving while this request is in flight.
        let _: [ChatMessage] = try await call("chatThread", ["friendId": friendId])
    }
    func closeChat(friendId: String) async throws { try await action("closeChat", ["friendId": friendId]) }
    func sendChatText(friendId: String, text: String, replyTo: String? = nil) async throws {
        var args: [String: Any] = ["friendId": friendId, "text": text]
        if let replyTo { args["replyTo"] = replyTo }
        try await action("sendChatText", args)
    }
    func sendChatFiles(friendId: String, source: String) async throws {
        let paths = try await pickFiles(source: source)
        guard !paths.isEmpty, chatPath.last == friendId else { return }
        // Staging is a separate action AFTER the picker reply, including resume.
        try await action("stageChatFiles", ["friendId": friendId, "paths": paths])
    }
    func pickFiles(source: String) async throws -> [String] {
        guard mediaTask == nil else { throw failure("A selection is still finishing. Please try again in a moment.") }
        let token = UUID(); mediaToken = token
        preparingMedia = source == "photos" ? "Preparing photo…" : source == "folder" ? "Preparing folder…" : "Preparing files…"
        let task = Task<[String], Error> {
            try await NativePresentation.waitForPickerDismissal()
            // A whole folder is picked natively (a temporary copy the engine can read).
            if source == "folder" { return try await NativeFolderPicker.shared.pickFolderToSend().map { [$0] } ?? [] }
            return try await call("pickFiles", ["source": source])
        }
        mediaTask = task
        defer { if mediaToken == token { mediaTask = nil; preparingMedia = nil; mediaToken = nil } }
        return try await task.value
    }
    func cancelMediaPreparation() {
        mediaTask?.cancel()
        preparingMedia = nil
    }
    func pickAvatar() async throws {
        let paths = try await pickFiles(source: "photos")
        guard let path = paths.first else { return }
        try await action("setAvatar", ["path": path])
    }
    func browserUpload(_ args: [String: Any]) async throws {
        var request = args
        if let source = args["source"] as? String, source != "folder" {
            let paths = try await pickFiles(source: source)
            guard !paths.isEmpty else { return }
            request["paths"] = paths
        }
        try await action("browserUpload", request)
    }
    func removeChatDraftFile(path: String) async throws { try await action("removeChatDraftFile", ["path": path]) }
    func reactToMessage(friendId: String, messageId: String, emoji: String) async throws { try await action("reactToMessage", ["friendId": friendId, "messageId": messageId, "emoji": emoji]) }
    func editMessage(friendId: String, messageId: String, text: String) async throws { try await action("editMessage", ["friendId": friendId, "messageId": messageId, "text": text]) }
    func deleteMessage(friendId: String, messageId: String) async throws { try await action("deleteMessage", ["friendId": friendId, "messageId": messageId]) }
    func markChatRead(friendId: String) async throws { try await action("markChatRead", ["friendId": friendId]) }
    func setTyping(friendId: String, on: Bool) async throws { try await action("setTyping", ["friendId": friendId, "bool": on]) }
    func nativeChatFocus(_ focused: Bool) async throws { try await action("nativeChatFocus", ["bool": focused]) }
    func retryChatFile(friendId: String, messageId: String) async throws { try await action("retryChatFile", ["friendId": friendId, "messageId": messageId]) }
    func openChatFile(path: String) async throws { try await action("openChatFile", ["path": path]) }
    func chatGifs(query: String) async throws -> [GifResult] { try await call("chatGifs", ["query": query]) }
    func sendChatGif(friendId: String, id: String) async throws { try await action("sendChatGif", ["friendId": friendId, "id": id]) }
    func setView(name: String) async throws { try await action("setView", ["name": name]) }
    func pingFriend(id: String) async throws -> ConnectionCheck { try await call("pingFriend", ["id": id]) }
    func removeFriend(id: String) async throws { try await action("removeFriend", ["id": id]) }
    func renameFriend(id: String, name: String) async throws { try await action("renameFriend", ["id": id, "name": name]) }
    func setAutoAccept(id: String, bool: Bool) async throws { try await action("setAutoAccept", ["id": id, "bool": bool]) }
    func myInviteCode() async throws -> String { try await call("myInviteCode") }
    func addFriendByCode(code: String) async throws { try await action("addFriendByCode", ["code": code]) }
    func acceptFriend(code: String) async throws { try await action("acceptFriend", ["code": code]) }
    func shareFiles(paths: [String]) async throws { try await action("shareFiles", ["paths": paths]) }
    func linkDeviceBegin() async throws -> String { try await call("linkDeviceBegin") }
    func linkDeviceCancel() async throws { try await action("linkDeviceCancel") }
    func linkDeviceSend(code: String) async throws -> LinkResult { try await call("linkDeviceSend", ["code": code]) }
    func linkHostBegin() async throws -> String { try await call("linkHostBegin") }
    func linkHostCancel() async throws { try await action("linkHostCancel") }
    func linkDeviceJoin(code: String) async throws -> LinkResult { try await call("linkDeviceJoin", ["code": code]) }
    func accountSyncNow() async throws { try await action("accountSyncNow") }
    func accountRemoveDevice(endpointId: String) async throws { try await action("accountRemoveDevice", ["endpointId": endpointId]) }
    func accountLeave() async throws { try await action("accountLeave") }
    /// True for the two device-link codes (either direction).
    static func isLinkCode(_ code: String) -> Bool {
        let c = code.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return c.hasPrefix("dropbeamjoin1:") || c.hasPrefix("dropbeamlink1:")
    }
    /// Link with whichever device-link code was scanned: a code shown by a device
    /// that has the account ("dropbeamjoin1:") makes THIS device join it; a code
    /// shown by a new device ("dropbeamlink1:") adds that device to this account.
    func linkWithScannedCode(_ code: String) async throws -> LinkResult {
        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.lowercased().hasPrefix("dropbeamjoin1:") { return try await linkDeviceJoin(code: trimmed) }
        if trimmed.lowercased().hasPrefix("dropbeamlink1:") { return try await linkDeviceSend(code: trimmed) }
        throw NSError(domain: "DropBeam", code: 1, userInfo: [NSLocalizedDescriptionKey: "That isn't a DropBeam device code. On your other device open Settings → Devices."])
    }
    func updateSettings(patch: [String: Any]) async throws { try await action("updateSettings", ["patch": patch]) }
    func respondToOffer(id: String, accept: Bool) async throws { try await action("respondToOffer", ["id": id, "accept": accept]) }
}
