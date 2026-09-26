import CryptoKit
import Foundation
import ObjectiveC
import UIKit
import UserNotifications

/// Transfer Server push (docs/TRANSFER-SERVER-PLAN.md §6).
///
/// 1. A private X25519 "push key" is created once and kept in the App Group
///    container, where the Notification Service Extension (DropBeamNotify) can
///    read it to open sealed previews. Its public half goes to the engine, which
///    advertises it (signed) to friends so their apps can seal previews for us.
/// 2. We ask iOS for an APNs token. Tauri owns the app delegate, so the token
///    callback is added to the delegate class at runtime.
/// 3. The token + public key are written to `push-token.json` in the engine's
///    config folder; the engine seals the token for the push relay and registers
///    it with the Transfer Servers that hold our messages.
///
/// Before the owner has enabled push (no `aps-environment` entitlement / APNs
/// key), registration simply fails and nothing else changes.
enum PushRegistration {
    static let appGroup = "group.com.ashtonmiller.dropbeam"
    private static var started = false

    static func start() {
        guard !started else { return }
        started = true
        guard pushKey() != nil else { return }
        installDelegateHooks()
        Task { @MainActor in
            let settings = await UNUserNotificationCenter.current().notificationSettings()
            // Never prompt from here: the app's own onboarding asks for notifications.
            guard settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional else { return }
            UIApplication.shared.registerForRemoteNotifications()
        }
    }

    /// Call when notification permission was just granted.
    static func permissionGranted() {
        guard started, pushKey() != nil else { return }
        Task { @MainActor in UIApplication.shared.registerForRemoteNotifications() }
    }

    // ── push key (shared with the extension) ────────────────────────────────

    static var keyURL: URL? {
        FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
            .appendingPathComponent("push-key", isDirectory: false)
    }

    /// Load or create the device push key. Nil when the App Group isn't
    /// available (then push stays off; everything else works).
    static func pushKey() -> Curve25519.KeyAgreement.PrivateKey? {
        guard let url = keyURL else { return nil }
        if let raw = try? Data(contentsOf: url), let key = try? Curve25519.KeyAgreement.PrivateKey(rawRepresentation: raw) {
            return key
        }
        let key = Curve25519.KeyAgreement.PrivateKey()
        do {
            // Readable after the first unlock, so a locked phone can still show previews.
            try key.rawRepresentation.write(to: url, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        } catch {
            return nil
        }
        return key
    }

    /// endpoint id → friend name, for the extension: a banner's title always
    /// comes from YOUR contacts, never from what a sender claims.
    static func saveNames(_ pairs: [(String, String)]) {
        guard let dir = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: appGroup) else { return }
        var map: [String: String] = [:]
        for (eid, name) in pairs { map[eid] = name }
        guard let data = try? JSONSerialization.data(withJSONObject: map) else { return }
        try? data.write(to: dir.appendingPathComponent("push-names.json"), options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
    }

    // ── token → engine ──────────────────────────────────────────────────────

    /// The engine's config folder (tauri app_config_dir on iOS).
    static var engineDir: URL? {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first?
            .appendingPathComponent(Bundle.main.bundleIdentifier ?? "com.ashtonmiller.dropbeam", isDirectory: true)
    }

    static func deliver(token: Data) {
        guard let key = pushKey(), let dir = engineDir else { return }
        #if DEBUG
        let env = "sandbox"
        #else
        let env = "prod"
        #endif
        let body: [String: Any] = [
            "token": token.map { String(format: "%02x", $0) }.joined(),
            "env": env,
            "pushKey": key.publicKey.rawRepresentation.base64EncodedString(),
        ]
        guard let data = try? JSONSerialization.data(withJSONObject: body) else { return }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? data.write(to: dir.appendingPathComponent("push-token.json"), options: .atomic)
    }

    // ── app delegate hooks (Tauri owns the delegate) ────────────────────────

    private static func installDelegateHooks() {
        guard let delegate = UIApplication.shared.delegate else { return }
        let cls: AnyClass = type(of: delegate)
        let didRegister = #selector(UIApplicationDelegate.application(_:didRegisterForRemoteNotificationsWithDeviceToken:))
        let didFail = #selector(UIApplicationDelegate.application(_:didFailToRegisterForRemoteNotificationsWithError:))
        let register: @convention(block) (AnyObject, UIApplication, Data) -> Void = { _, _, token in
            PushRegistration.deliver(token: token)
        }
        let fail: @convention(block) (AnyObject, UIApplication, Error) -> Void = { _, _, error in
            NSLog("DropBeam push: not available (%@)", error.localizedDescription)
        }
        // "v@:@@" = void, self, _cmd, UIApplication, arg. Only add — never replace
        // a delegate's own implementation.
        if !class_respondsToSelector(cls, didRegister) {
            class_addMethod(cls, didRegister, imp_implementationWithBlock(register), "v@:@@")
        }
        if !class_respondsToSelector(cls, didFail) {
            class_addMethod(cls, didFail, imp_implementationWithBlock(fail), "v@:@@")
        }
        // UIKit caches which optional delegate methods exist when the delegate is
        // set; re-assign it so the new ones are seen.
        let app = UIApplication.shared
        app.delegate = nil
        app.delegate = delegate
    }
}
