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
/// Anything unexpected leaves the generic banner as it is.
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
        if let sealed = request.content.userInfo["e"] as? String, !sealed.isEmpty,
           let preview = Self.open(sealed) {
            if let t = preview["t"] as? String, !t.isEmpty { best.title = t }
            if let b = preview["b"] as? String, !b.isEmpty { best.body = b }
        }
        contentHandler(best)
    }

    override func serviceExtensionTimeWillExpire() {
        if let handler, let content { handler(content) }
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
