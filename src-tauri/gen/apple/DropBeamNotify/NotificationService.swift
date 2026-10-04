import CryptoKit
import Foundation
import UserNotifications

/// Opens the sealed preview a Transfer Server push carries ("e") with this
/// phone's push key (shared by the app through the App Group), so the banner
/// reads "Ashton — running 10 min late" instead of "New message". The server
/// and the push relay only ever see the sealed bytes.
///
/// Layout (Rust `mailbox::seal::seal_small`): eph_pub(32) ‖ nonce(12) ‖ ct ‖ tag,
/// key = HKDF-SHA256(X25519(push_key, eph_pub), salt: eph_pub ‖ my_pub,
/// info: "dropbeam-push-v1"), ChaCha20-Poly1305. Plaintext {"t","b","th"}.
/// Plaintext {"t","b","f","i"} ("i" = message id, newer senders). Anything
/// unexpected leaves the generic banner.
final class NotificationService: UNNotificationServiceExtension {
    private var handler: ((UNNotificationContent) -> Void)?
    private var content: UNMutableNotificationContent?

    override func didReceive(_ request: UNNotificationRequest, withContentHandler contentHandler: @escaping (UNNotificationContent) -> Void) {
        handler = contentHandler
        guard let best = request.content.mutableCopy() as? UNMutableNotificationContent else {
            contentHandler(request.content)
            return
        }
        content = best
        // "e" = {"f": sender (checked by the server against the item's signature),
        //        "e": preview sealed by that sender to this phone}.
        if let outer = request.content.userInfo["e"] as? String,
           let wrap = try? JSONSerialization.jsonObject(with: Data(outer.utf8)) as? [String: Any],
           let from = wrap["f"] as? String, let sealed = wrap["e"] as? String,
           let name = Self.names()[from] {
            // The title is always YOUR name for them; the text only if the sealed
            // preview really is from that sender.
            best.title = name
            // Tapping the banner opens this person's conversation (the app reads
            // `__EXTRA__.chatPeerId` like its own chat banners), grouped per person.
            if let chat = Self.peers()[from] {
                var info = best.userInfo
                info["__EXTRA__"] = ["chatPeerId": chat]
                best.userInfo = info
                best.threadIdentifier = "chat-" + chat
            }
            if let preview = Self.open(sealed), preview["f"] as? String == from {
                if let b = preview["b"] as? String, !b.isEmpty {
                    best.body = b
                }
                // One banner per message: the app (or an earlier push) already
                // has this one → deliver it silently, without alerting again.
                if let id = preview["i"] as? String, !id.isEmpty {
                    if Self.ids("app-have.json")[id] != nil || Self.ids("nse-notified.json")[id] != nil {
                        best.sound = nil
                        if #available(iOS 15.0, *) { best.interruptionLevel = .passive }
                    } else {
                        Self.remember(id)
                    }
                }
            }
        }
        contentHandler(best)
    }

    /// {message id: ms} lists shared with the app through the App Group.
    static func ids(_ name: String) -> [String: Any] {
        guard let url = group?.appendingPathComponent(name),
              let data = try? Data(contentsOf: url),
              let map = try? JSONSerialization.jsonObject(with: data) as? [String: Any] else { return [:] }
        return map
    }

    /// Note that this message was announced, so the app doesn't ring again
    /// when it later fetches or syncs the same message.
    static func remember(_ id: String) {
        guard let url = group?.appendingPathComponent("nse-notified.json") else { return }
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        func at(_ v: Any) -> Int64 { (v as? NSNumber)?.int64Value ?? 0 }
        var map = ids("nse-notified.json").filter { now - at($0.value) < 7 * 86_400_000 }
        map[id] = NSNumber(value: now)
        if map.count > 2000 {
            let keep = map.sorted { at($0.value) > at($1.value) }.prefix(2000)
            map = Dictionary(uniqueKeysWithValues: keep.map { ($0.key, $0.value) })
        }
        if let data = try? JSONSerialization.data(withJSONObject: map) {
            try? data.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        }
    }

    override func serviceExtensionTimeWillExpire() {
        if let handler, let content { handler(content) }
    }

    static var group: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.com.ashtonmiller.dropbeam")
    }

    /// endpoint id → chat (thread) id, written by the app (PushRegistration.savePeers).
    static func peers() -> [String: String] {
        guard let url = group?.appendingPathComponent("push-peers.json"),
              let data = try? Data(contentsOf: url),
              let map = try? JSONSerialization.jsonObject(with: data) as? [String: String] else { return [:] }
        return map
    }

    static func names() -> [String: String] {
        guard let url = group?.appendingPathComponent("push-names.json"),
              let data = try? Data(contentsOf: url),
              let map = try? JSONSerialization.jsonObject(with: data) as? [String: String] else { return [:] }
        return map
    }

    static func open(_ sealed: String) -> [String: Any]? {
        guard let raw = Data(base64Encoded: sealed), raw.count > 32 + 12 + 16,
              let url = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.com.ashtonmiller.dropbeam")?
                .appendingPathComponent("push-key"),
              let keyData = try? Data(contentsOf: url),
              let me = try? Curve25519.KeyAgreement.PrivateKey(rawRepresentation: keyData),
              let eph = try? Curve25519.KeyAgreement.PublicKey(rawRepresentation: raw.prefix(32)),
              let shared = try? me.sharedSecretFromKeyAgreement(with: eph)
        else { return nil }
        let salt = raw.prefix(32) + me.publicKey.rawRepresentation
        let key = shared.hkdfDerivedSymmetricKey(using: SHA256.self, salt: salt, sharedInfo: Data("dropbeam-push-v1".utf8), outputByteCount: 32)
        guard let box = try? ChaChaPoly.SealedBox(combined: raw.dropFirst(32)),
              let plain = try? ChaChaPoly.open(box, using: key),
              let obj = try? JSONSerialization.jsonObject(with: plain) as? [String: Any]
        else { return nil }
        return obj
    }
}
