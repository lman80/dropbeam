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
        // Items shared to DropBeam from another app wait in the App Group until we
        // come forward (the share extension also opens us with dropbeam://share).
        NotificationCenter.default.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
            Task { @MainActor in ShareInbox.shared.ingestSoon() }
        }
    }
    @Published var history: [HistoryEntry] = []
    @Published var locations: [FriendLocations] = []
    @Published var needsName = false
    @Published var pendingSend: [String] = []
    /// Paths the user just picked on the Send tab, owned by Swift. The web store's
    /// `pendingSend` snapshot (re-pushed right after every pick reply) must not be able to
    /// clear them before the Send To sheet appears — that race left the sheet unshown.
    @Published var pickedToSend: [String] = []
    /// What the Send To sheet offers: a fresh pick, else an engine/share-sheet request.
    var sendQueue: [String] { pickedToSend.isEmpty ? pendingSend : pickedToSend }
    @Published var folderInvites: [FolderInvite] = []
    @Published var toast: String?
    @Published var friends: [Friend] = []
    @Published var myDevice: MyDevice?
    @Published var transfers: [Transfer] = []
    /// Live transfers on the user's other linked devices (read-only, #31).
    @Published var otherDevices: [DeviceActivity] = []
    @Published var settings: Settings?
    @Published var chatOverview: [ChatOverview] = []
    @Published var chatUnread: [String: Int] = [:] { didSet { updateAppBadge(); clearReadBanners(was: oldValue) } }
    @Published var folders: [SharedFolder] = []
    @Published var blocked: [BlockedPerson] = []
    @Published var chatTyping: [String: Bool] = [:]
    /// Transfer Servers this device may use (iOS only uses them, never hosts).
    @Published var servers: [UsableServer] = []
    /// Files friends sent through a server that wait for this user's OK.
    @Published var pendingFiles: [PendingFile] = []
    /// Per thread: the server a message would wait on while the friend is offline.
    @Published var holdRoutes: [String: String] = [:]
    /// Can servers wake this iPhone, and do those notifications show message text?
    @Published var pushStatus = PushStatus()
    /// Simulator-only QA data is showing; engine snapshots for these keys are ignored.
    var previewKeys: Set<String> = []
    @Published var threads: [String: [ChatMessage]] = [:]
    @Published var chatDraftFiles: [String] = []
    @Published var chatPath: [String] = []
    @Published var presence: [String: Bool] = [:]
    /// Last contact (ms since 1970) for people who are not online right now.
    @Published var presenceSeen: [String: Double] = [:]
    @Published var selectedTab = "send"
    @Published var errorMessage: String?
    /// Opens Settings → Devices (the only place a device-link code is used).
    @Published var showDevices = false
    /// People asking to be friends (S2): not friends until accepted.
    @Published var friendRequests: [FriendRequest] = []
    /// A `dropbeam:` link the app was opened with (invite link, Camera-scanned QR).
    @Published var incomingLink: IncomingLink?
    /// First-run setup is showing (persisted, so a relaunch mid-setup resumes it).
    @Published var onboarding = UserDefaults.standard.bool(forKey: "dropbeam.onboarding.pending") {
        didSet { UserDefaults.standard.set(onboarding, forKey: "dropbeam.onboarding.pending") }
    }
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
    /// A chat read here (or on another of your devices) shouldn't leave its
    /// banners in Notification Center: drop the ones for chats now at 0.
    private func clearReadBanners(was old: [String: Int]) {
        let cleared = Set(old.filter { $0.value > 0 && (chatUnread[$0.key] ?? 0) == 0 }.map(\.key))
        guard !cleared.isEmpty else { return }
        let center = UNUserNotificationCenter.current()
        center.getDeliveredNotifications { delivered in
            let ids = delivered.filter { n in
                let content = n.request.content
                let thread = content.threadIdentifier.hasPrefix("chat-") ? String(content.threadIdentifier.dropFirst(5)) : nil
                let extra = (content.userInfo["chatPeerId"] as? String)
                    ?? ((content.userInfo["extra"] as? [String: Any])?["chatPeerId"] as? String)
                return [thread, extra].compactMap { $0 }.contains(where: cleared.contains)
            }.map(\.request.identifier)
            if !ids.isEmpty { center.removeDeliveredNotifications(withIdentifiers: ids) }
        }
    }

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
            let seconds: Double = ["pickFiles", "sendChatFiles"].contains(name) ? 1800 : ["setAvatar", "browserUpload", "acceptFolderInvite"].contains(name) ? 1800 : name == "eraseAllData" ? 60 : name.hasPrefix("browser") || ["locationsList", "locationsRefresh"].contains(name) ? 180 : 30
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
    /// A store snapshot decoded OFF the main thread (NativeUIPlugin.state), applied here.
    enum Snapshot {
        case history([HistoryEntry]), locations([FriendLocations]), needsName(Bool), pendingSend([String])
        case friends([Friend]), myDevice(MyDevice?), transfers([Transfer]), settings(Settings?)
        case chatOverview([ChatOverview]), chatUnread([String: Int]), chatTyping([String: Bool]), chatDraftFiles([String])
        case thread(ChatThread?), presence([String: Bool]), presenceSeen([String: Double]), folders([SharedFolder])
        case blocked([BlockedPerson]), servers([UsableServer]), pendingFiles([PendingFile]), otherDevices([DeviceActivity])
        case unknown
    }
    private struct Envelope<T: Decodable>: Decodable { let value: T }
    private struct KeyOnly: Decodable { let key: String }
    /// Decode `{"key": …, "value": …}` straight from the plugin's raw arguments.
    nonisolated static func decodeSnapshot(raw: Data) throws -> (key: String, snapshot: Snapshot) {
        let decoder = JSONDecoder()
        let key = try decoder.decode(KeyOnly.self, from: raw).key
        func value<T: Decodable>(_: T.Type) throws -> T { try decoder.decode(Envelope<T>.self, from: raw).value }
        let snapshot: Snapshot
        switch key {
        case "history": snapshot = .history(try value(LossyArray<HistoryEntry>.self).values)
        case "locations": snapshot = .locations(try value(LossyArray<FriendLocations>.self).values)
        case "needsName": snapshot = .needsName(try value(Bool.self))
        case "pendingSend": snapshot = .pendingSend(try value(LossyArray<String>.self).values)
        case "friends": snapshot = .friends(try value(LossyArray<Friend>.self).values)
        case "myDevice": snapshot = .myDevice(try value(MyDevice?.self))
        case "transfers": snapshot = .transfers(try value(LossyArray<Transfer>.self).values)
        case "settings": snapshot = .settings(try value(Settings?.self))
        case "chatOverview": snapshot = .chatOverview(try value(LossyArray<ChatOverview>.self).values)
        case "chatUnread": snapshot = .chatUnread(try value([String: Int].self))
        case "chatTyping": snapshot = .chatTyping(try value([String: Bool].self))
        case "chatDraftFiles": snapshot = .chatDraftFiles(try value(LossyArray<String>.self).values)
        case "thread": snapshot = .thread(try value(ChatThread?.self))
        case "presence": snapshot = .presence(try value([String: Bool].self))
        case "presenceSeen": snapshot = .presenceSeen(try value([String: Double].self))
        case "folders": snapshot = .folders(try value(LossyArray<SharedFolder>.self).values)
        case "blocked": snapshot = .blocked(try value(LossyArray<BlockedPerson>.self).values)
        case "transferServers": snapshot = .servers(try value(LossyArray<UsableServer>.self).values)
        case "pendingFiles": snapshot = .pendingFiles(try value(LossyArray<PendingFile>.self).values)
        case "otherDevices": snapshot = .otherDevices(try value(LossyArray<DeviceActivity>.self).values.filter { !$0.items.isEmpty })
        default: snapshot = .unknown // Forward-compatible snapshots.
        }
        return (key, snapshot)
    }
    /// Main-thread half: assign + the side effects some snapshots carry.
    func apply(key: String, _ snapshot: Snapshot) {
        if previewKeys.contains(key) { return }
        switch snapshot {
        case .history(let v): history = v
        case .locations(let v): locations = v
        case .needsName(let v):
            needsName = v
            // Only a brand-new install is asked its name: that starts first-run setup,
            // which then stays up (name saved or not) until the user finishes it.
            if needsName && !onboarding { onboarding = true }
        case .pendingSend(let v): pendingSend = v
        case .friends(let v):
            friends = v
            // SuperFeedback moment of value: a new friend (not the launch snapshot).
            if let known = knownFriends, !launching, friends.contains(where: { !known.contains($0.id) }) { SuperFeedback.moment("friend-added") }
            knownFriends = Set(friends.map(\.id))
            PushRegistration.saveNames(friends.compactMap { f in f.endpointId.map { ($0, f.name) } })
            PushRegistration.savePeers(friends.compactMap { f in f.endpointId.map { ($0, f.id) } })
        case .myDevice(let v): myDevice = v
        case .transfers(let v):
            transfers = v
            reportTransferMoments()
        case .settings(let v):
            let first = settings == nil && v != nil
            settings = v
            // The engine is up: anything the share extension left (its "open DropBeam"
            // can fail, or the app was launched cold) goes out now, not next foreground.
            if first { ShareInbox.shared.ingestSoon() }
            if let share = settings?.shareDiagnostics {
                UserDefaults.standard.set(share, forKey: NativeUIPlugin.diagnosticsKey)
                SuperFeedback.setCrashReportingEnabled(share)
                CrashDiagnostics.shared.setEnabled(share)
            }
            SaveFolder.shared.sync(engineDir: settings?.downloadDir)
        case .chatOverview(let v): chatOverview = v
        case .chatUnread(let v): chatUnread = v
        case .chatTyping(let v): chatTyping = v
        case .chatDraftFiles(let v): chatDraftFiles = v
        case .thread(let v): if let thread = v { threads[thread.friendId] = thread.messages }
        case .presence(let v): presence = v
        case .presenceSeen(let v): presenceSeen = v
        case .folders(let v): folders = v
        case .blocked(let v): blocked = v
        case .servers(let v): servers = v
        case .pendingFiles(let v): pendingFiles = v
        case .otherDevices(let v): otherDevices = v
        case .unknown: break
        }
        // The share extension lists friends from a snapshot in the App Group.
        if ["friends", "presence", "myDevice"].contains(key) { ShareInbox.shared.recipientsChanged() }
    }
    /// Synchronous variant (previews/tests): decode + apply on the main thread.
    func update(key: String, value: Any) throws {
        let raw = try JSONSerialization.data(withJSONObject: ["key": key, "value": value], options: [.fragmentsAllowed])
        let decoded = try Self.decodeSnapshot(raw: raw)
        apply(key: decoded.key, decoded.snapshot)
    }
    func event(name: String, payload: Any) {
        let object = payload as? [String: Any] ?? [:]
        // Simulator QA data owns navigation (the engine doesn't know the seeded friends).
        if previewKeys.contains("thread") && ["chatOpen", "view"].contains(name) { return }
        if name == "view", let tab = object["name"] as? String,
           ["send", "friends", "chat", "history", "settings"].contains(tab) { selectedTab = tab }
        if name == "error" { errorMessage = object["message"] as? String }
        if name == "friend-requests://changed" { Task { await refreshFriendRequests() } }
        if name == "openURL", let url = object["url"] as? String {
            if ShareInbox.isShareURL(url) { ShareInbox.shared.ingestSoon() }
            else { incomingLink = IncomingLink(value: url) }
        }
        if name == "received://files", let paths = object["paths"] as? [String] {
            ReceivedMediaSaver.shared.received(paths: paths, chat: object["chat"] as? Bool ?? false)
        }
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
        else { pickedToSend = paths }
    }
    private var knownFriends: Set<String>?
    /// The first seconds after launch replay stored state (friends, finished transfers) in
    /// several snapshots: none of that is a new moment of value.
    private let launchedAt = Date()
    private var launching: Bool { Date().timeIntervalSince(launchedAt) < 15 }
    /// Transfers already counted as finished; nil until the first snapshot (history, not news).
    private var finishedTransfers: Set<String>?
    /// SuperFeedback moments of value: files sent / received, a file delivered in a chat.
    private func reportTransferMoments() {
        let done = transfers.filter { $0.state == "completed" }
        defer { finishedTransfers = (finishedTransfers ?? []).union(done.map(\.id)) }
        guard let seen = finishedTransfers else { return }
        // Something finished: copies made only for sending may be cleaned up now.
        if done.contains(where: { !seen.contains($0.id) }) { PickedMedia.sweepSoon(after: 30) }
        guard !launching else { return }
        for transfer in done where !seen.contains(transfer.id) {
            if transfer.chatOnly == true { if transfer.direction == "send" { SuperFeedback.moment("chat-file-delivered") } }
            else { SuperFeedback.moment(transfer.direction == "send" ? "files-sent" : "files-received") }
        }
    }
    func showToast(_ message: String) {
        toast = message
        Task { try? await Task.sleep(for: .seconds(5)); if toast == message { toast = nil } }
    }
    func sendToFriend(friendId: String, paths: [String], device: String? = nil) async throws {
        var args: [String: Any] = ["friendId": friendId, "paths": paths]
        if let device { args["device"] = device }
        BackgroundTransfers.shared.userStartedSend(paths: paths, to: friends.first { $0.id == friendId }?.displayName)
        try await action("sendToFriend", args)
    }
    func receiveWithCode(code: String) async throws { try await action("receiveWithCode", ["code": code]) }
    /// Any DropBeam code (Quick Send, friend, friend invite, folder invite), routed
    /// like desktop's "Have a code?". Folder invites open the folder picker here.
    /// Device-link codes are refused (S1): only Settings → Devices links a device.
    func openAnyCode(_ code: String) async throws {
        if Bridge.isLinkCode(code) { throw failure(Bridge.deviceCodeElsewhere) }
        let result: OpenCodeResult = try await call("openAnyCode", ["code": code])
        switch result.kind {
        case "friend": showToast("Friend added")
        case "folderInvite":
            let accepted: Bool = try await call("acceptFolderInvite", ["code": result.code ?? code])
            if accepted { showToast("Joined shared folder") }
        default: break
        }
    }
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
        // Staged attachments ride along with this send (a big video keeps going in the background).
        if !chatDraftFiles.isEmpty { BackgroundTransfers.shared.userStartedSend(paths: chatDraftFiles, to: friends.first { $0.id == friendId }?.displayName) }
        var args: [String: Any] = ["friendId": friendId, "text": text]
        if let replyTo { args["replyTo"] = replyTo }
        try await action("sendChatText", args)
    }
    func sendChatFiles(friendId: String, source: String) async throws {
        let paths = try await pickFiles(source: source, purpose: .chat)
        guard !paths.isEmpty, chatPath.last == friendId else { return }
        // Staging is a separate action AFTER the picker reply, including resume.
        try await action("stageChatFiles", ["friendId": friendId, "paths": paths])
    }
    /// Photos / Files / a folder, copied into PickedMedia (Application Support, out of
    /// Files and backups, swept once sent). `purpose` .chat keeps copies longer: the
    /// conversation shows them.
    func pickFiles(source: String, purpose: PickedMedia.Purpose = .send) async throws -> [String] {
        guard mediaTask == nil else { throw failure("A selection is still finishing. Please try again in a moment.") }
        let token = UUID(); mediaToken = token
        let task = Task<[String], Error> {
            try await NativePresentation.waitForPickerDismissal()
            switch source {
            // A whole folder is picked natively (a private copy the engine can read).
            case "folder":
                preparingMedia = "Preparing folder…"
                return try await NativeFolderPicker.shared.pickFolderToSend(purpose: purpose).map { [$0] } ?? []
            case "photos":
                let result = try await NativePhotoPicker.shared.pick(purpose: purpose) { [weak self] done, total in
                    guard let self, self.mediaToken == token else { return }
                    self.preparingMedia = total > 1 ? "Preparing \(min(done + 1, total)) of \(total)…" : "Preparing photo…"
                }
                if result.failed > 0 {
                    showToast(result.failed == 1 ? "1 item couldn’t be loaded" : "\(result.failed) items couldn’t be loaded")
                }
                return result.paths
            default:
                // The overlay appears only once the picker hands files over (not while browsing).
                return try await NativeFolderPicker.shared.pickFiles(purpose: purpose) { [weak self] in
                    if self?.mediaToken == token { self?.preparingMedia = "Preparing files…" }
                }
            }
        }
        mediaTask = task
        defer { if mediaToken == token { mediaTask = nil; preparingMedia = nil; mediaToken = nil } }
        let paths = try await task.value
        if Task.isCancelled || task.isCancelled { throw CancellationError() }
        return paths
    }
    func cancelMediaPreparation() {
        mediaTask?.cancel()
        // Free the slot now: a load that ignores cancellation must not block later picks.
        mediaTask = nil; mediaToken = nil
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
    /// Dial every device of this person now; true as soon as any answers.
    func checkPresence(id: String) async throws -> Bool {
        let check: ConnectionCheck = try await call("checkPresence", ["id": id])
        return check.online == true
    }
    func removeFriend(id: String) async throws { try await action("removeFriend", ["id": id]) }
    /// Block the person behind friend `id` (all their devices, on all your devices).
    func blockFriend(id: String) async throws { try await action("blockFriend", ["id": id]) }
    func unblockPerson(id: String) async throws { try await action("unblockPerson", ["id": id]) }
    func reportReasons() async throws -> [ReportReason] { try await call("reportReasons") }
    /// Build a report email: only the fields passed are included (never files).
    func reportMail(_ args: [String: Any]) async throws -> ReportMail { try await call("reportMail", args) }
    func renameFriend(id: String, name: String) async throws { try await action("renameFriend", ["id": id, "name": name]) }
    func setAutoAccept(id: String, bool: Bool) async throws { try await action("setAutoAccept", ["id": id, "bool": bool]) }
    func myInviteCode() async throws -> String { try await call("myInviteCode") }
    func addFriendByCode(code: String) async throws { try await action("addFriendByCode", ["code": code]) }
    func acceptFriend(code: String) async throws { try await action("acceptFriend", ["code": code]) }
    func shareFiles(paths: [String]) async throws { try await action("shareFiles", ["paths": paths]) }
    func linkDeviceBegin() async throws -> String { try await call("linkDeviceBegin") }
    func linkDeviceCancel() async throws { try await action("linkDeviceCancel") }
    /// Step 1 with the other device's code: its name and the safety code to compare.
    func linkDevicePrepare(code: String) async throws -> LinkPreviewInfo { try await call("linkDevicePrepare", ["code": code]) }
    /// This device's answer to a `link://confirm` (the other device scanned ours).
    func linkConfirm(endpointId: String, accept: Bool) async throws { try await action("linkConfirm", ["endpointId": endpointId, "bool": accept]) }
    /// Step 2, only after the user confirmed the safety code (the engine refuses otherwise).
    func linkDeviceSend(code: String) async throws -> LinkResult { try await call("linkDeviceSend", ["code": code]) }
    func linkHostBegin() async throws -> String { try await call("linkHostBegin") }
    func linkHostCancel() async throws { try await action("linkHostCancel") }
    func accountSyncNow() async throws { try await action("accountSyncNow") }
    func accountRemoveDevice(endpointId: String) async throws { try await action("accountRemoveDevice", ["endpointId": endpointId]) }
    /// S4: vouch for a device that proves the account key but wasn't approved by a linked device.
    func accountApproveDevice(endpointId: String) async throws { try await action("accountApproveDevice", ["endpointId": endpointId]) }
    func accountLeave() async throws { try await action("accountLeave") }
    /// True for the two device-link codes (either direction).
    static func isLinkCode(_ code: String) -> Bool {
        let c = code.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        return c.hasPrefix("dropbeamjoin1:") || c.hasPrefix("dropbeamlink1:")
    }
    /// What every entry point but Settings → Devices says about a device-link code
    /// (the same words nativeBridge's openAnyCode throws).
    static let deviceCodeElsewhere = "This is a device-link code — open Settings → Devices to link your own device."
    static func isDeviceCodeMessage(_ message: String?) -> Bool { message?.contains("device-link code") == true }
    /// Settings → Devices, from anywhere (e.g. after a device code was used elsewhere).
    func openDevicesSettings() {
        selectedTab = "settings"
        Task { try? await Task.sleep(for: .milliseconds(350)); showDevices = true }
    }
    // MARK: Friend requests (S2)
    func refreshFriendRequests() async {
        if previewKeys.contains("friendRequests") { return }
        guard let list: LossyArray<FriendRequest> = try? await call("listFriendRequests") else { return }
        friendRequests = list.values.sorted { ($0.at ?? 0) > ($1.at ?? 0) }
    }
    func acceptFriendRequest(endpointId: String) async throws {
        try await action("acceptFriendRequest", ["endpointId": endpointId])
        friendRequests.removeAll { $0.endpointId == endpointId }
        await refreshFriendRequests()
    }
    /// Decline; `block` also blocks them on all your devices.
    func declineFriendRequest(endpointId: String, block: Bool) async throws {
        try await action("declineFriendRequest", ["endpointId": endpointId, "bool": block])
        friendRequests.removeAll { $0.endpointId == endpointId }
        await refreshFriendRequests()
    }
    func updateSettings(patch: [String: Any]) async throws { try await action("updateSettings", ["patch": patch]) }
    // MARK: Transfer Servers
    func setServerPrefs(eid: String, useIt: Bool? = nil, holdForMe: Bool? = nil, offer: String? = nil, shareFriends: Bool? = nil) async throws {
        var args: [String: Any] = ["eid": eid]
        if let shareFriends { args["shareFriends"] = shareFriends }
        if let useIt { args["useIt"] = useIt }
        if let holdForMe { args["holdForMe"] = holdForMe }
        if let offer { args["offer"] = offer }
        if previewKeys.contains("transferServers") { previewServerPrefs(args); return }
        servers = try await call("serverPrefs", args)
    }
    func forgetServer(eid: String) async throws {
        if previewKeys.contains("transferServers") { servers.removeAll { $0.eid == eid }; return }
        servers = try await call("serverForget", ["eid": eid])
    }
    /// Pull anything a Transfer Server holds for us (every return to the foreground).
    func mailboxFetchNow() async { try? await action("mailboxFetchNow") }
    func refreshHoldRoute(friendId: String) async {
        if previewKeys.contains("holdRoutes") { return }
        let route: String? = try? await call("serverHoldRoute", ["friendId": friendId])
        holdRoutes[friendId] = route.flatMap { $0.isEmpty ? nil : $0 }
    }
    func decidePendingFile(linkId: String, accept: Bool) async throws {
        if !previewKeys.contains("pendingFiles") { try await action("decidePendingFile", ["linkId": linkId, "accept": accept]) }
        pendingFiles.removeAll { $0.linkId == linkId }
    }
    func refreshPushStatus() async {
        if previewKeys.contains("pushStatus") { return }
        if let status: PushStatus = try? await call("pushStatus") { pushStatus = status }
    }
    func setPushPreviews(_ on: Bool) async throws {
        pushStatus.previews = on
        if previewKeys.contains("pushStatus") { return }
        do { pushStatus = try await call("pushSetPreviews", ["on": on]) }
        catch { pushStatus.previews = !on; throw error }
    }
    private func previewServerPrefs(_ args: [String: Any]) {
        guard let i = servers.firstIndex(where: { $0.eid == args["eid"] as? String }) else { return }
        if let v = args["useIt"] as? Bool { servers[i].useIt = v; if !v { servers[i].holdForMe = false } }
        if let v = args["holdForMe"] as? Bool { servers[i].holdForMe = v }
        if let v = args["shareFriends"] as? Bool { servers[i].shareFriends = v }
        servers[i].offer = args["offer"] as? String ?? "seen"
    }
    func respondToOffer(id: String, accept: Bool) async throws { try await action("respondToOffer", ["id": id, "accept": accept]) }
}

struct IncomingLink: Identifiable, Equatable {
    let id = UUID()
    let value: String
}
