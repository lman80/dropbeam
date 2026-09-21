import Foundation
import Combine
import WebKit

@MainActor
final class Bridge: ObservableObject {
    static let shared = Bridge()
    @Published var friends: [Friend] = []
    @Published var myDevice: MyDevice?
    @Published var transfers: [Transfer] = []
    @Published var settings: Settings?
    @Published var chatOverview: [ChatOverview] = []
    @Published var presence: [String: Bool] = [:]
    @Published var selectedTab = "send"
    @Published var errorMessage: String?
    weak var webview: WKWebView?
    private var nextID = 0
    private struct Pending {
        let continuation: CheckedContinuation<Data, Error>
        let timeout: DispatchWorkItem
    }
    private var pending: [Int: Pending] = [:]
    private let decoder = JSONDecoder()
    var unread: Int { chatOverview.reduce(0) { $0 + max(0, $1.unread ?? 0) } }

    func call<T: Decodable>(_ name: String, _ args: [String: Any] = [:]) async throws -> T {
        guard let webview else { throw failure("The app bridge is not ready.") }
        nextID += 1
        let id = nextID
        // Serialize all arguments, including the action name, as JSON; never
        // interpolate user text or paths into executable JavaScript.
        let encoded = try JSONSerialization.data(withJSONObject: [id, name, args])
        let json = String(decoding: encoded, as: UTF8.self)
        let data: Data = try await withCheckedThrowingContinuation { continuation in
            let timeout = DispatchWorkItem { [weak self] in
                self?.finish(id, .failure(self?.failure("This action timed out. Check its status before trying again.") ?? NSError(domain: "NativeUI", code: 1)))
            }
            pending[id] = Pending(continuation: continuation, timeout: timeout)
            DispatchQueue.main.asyncAfter(deadline: .now() + 20, execute: timeout)
            webview.evaluateJavaScript("window.__dbBridge.call(...\(json)); void 0") { [weak self] _, error in
                if let error { self?.finish(id, .failure(error)) }
            }
        }
        return try decoder.decode(T.self, from: data)
    }
    private func finish(_ id: Int, _ result: Result<Data, Error>) {
        guard let item = pending.removeValue(forKey: id) else { return }
        item.timeout.cancel()
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
        case "friends": friends = try decoder.decode([Friend].self, from: data)
        case "myDevice": myDevice = try decoder.decode(MyDevice?.self, from: data)
        case "transfers": transfers = try decoder.decode([Transfer].self, from: data)
        case "settings": settings = try decoder.decode(Settings?.self, from: data)
        case "chatOverview": chatOverview = try decoder.decode([ChatOverview].self, from: data)
        case "presence": presence = try decoder.decode([String: Bool].self, from: data)
        default: break // Forward-compatible snapshots.
        }
    }
    func event(name: String, payload: Any) {
        let object = payload as? [String: Any] ?? [:]
        if name == "view", let tab = object["name"] as? String,
           ["send", "friends", "chat", "history", "settings"].contains(tab) { selectedTab = tab }
        if name == "error" { errorMessage = object["message"] as? String }
        // Tauri events remain available to subsequent native chat/presence views;
        // authoritative published data always comes from the store snapshots.
        NotificationCenter.default.post(name: Notification.Name("DropBeam.\(name)"), object: nil, userInfo: ["payload": payload])
    }
    func perform(_ action: @escaping @MainActor () async throws -> Void) {
        Haptics.tap()
        Task { do { try await action() } catch { errorMessage = error.localizedDescription } }
    }
    private func failure(_ message: String) -> NSError { NSError(domain: "DropBeam.NativeUI", code: 1, userInfo: [NSLocalizedDescriptionKey: message]) }
    private func action(_ name: String, _ args: [String: Any] = [:]) async throws {
        let _: IgnoredResult = try await call(name, args)
    }
    func pickAndSend(source: String, friendId: String? = nil) async throws {
        var args: [String: Any] = ["source": source]
        if let friendId { args["friendId"] = friendId }
        try await action("pickAndSend", args)
    }
    func sendToFriend(friendId: String, paths: [String]) async throws { try await action("sendToFriend", ["friendId": friendId, "paths": paths]) }
    func receiveWithCode(code: String) async throws { try await action("receiveWithCode", ["code": code]) }
    func cancelTransfer(id: String) async throws { try await action("cancelTransfer", ["id": id]) }
    func retryTransfer(id: String) async throws { try await action("retryTransfer", ["id": id]) }
    func openChat(friendId: String) async throws { try await action("openChat", ["friendId": friendId]); selectedTab = "chat" }
    func sendChatText(friendId: String, text: String) async throws { try await action("sendChatText", ["friendId": friendId, "text": text]) }
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
    func updateSettings(patch: [String: Any]) async throws { try await action("updateSettings", ["patch": patch]) }
    func respondToOffer(id: String, accept: Bool) async throws { try await action("respondToOffer", ["id": id, "accept": accept]) }
}
