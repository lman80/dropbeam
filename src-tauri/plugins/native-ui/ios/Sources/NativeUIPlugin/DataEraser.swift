import Foundation
import UIKit
import UserNotifications
import WebKit

/// Settings → Privacy & Your Data → Erase All Data (App Review 5.1.1(v)).
///
/// DropBeam has no server account; "your data" is what this iPhone keeps. Erasing:
/// 1. the engine withdraws this phone's push token from every Transfer Server and, when
///    the phone is linked to the user's other devices, leaves that account (the other
///    devices keep everything and simply drop this iPhone);
/// 2. this iPhone stops receiving pushes, and every file DropBeam stored is deleted —
///    identity, friends, chats, history, settings, received files in the DropBeam folder,
///    picked/pasted copies, caches, the App Group (share extension, push key) and
///    preferences;
/// 3. the app closes; the next launch starts fresh, like a new install.
/// Files saved to a folder the user chose elsewhere (Save Files To) and items already
/// added to Photos are not touched.
@MainActor enum DataEraser {
    static func eraseAndQuit() async {
        let bridge = Bridge.shared
        // Best effort, bounded inside the JS handler (~40 s worst case).
        _ = try? await bridge.call("eraseAllData") as Bool
        UIApplication.shared.unregisterForRemoteNotifications()
        let center = UNUserNotificationCenter.current()
        center.removeAllPendingNotificationRequests()
        center.removeAllDeliveredNotifications()
        center.setBadgeCount(0) { _ in }
        await WKWebsiteDataStore.default().removeData(ofTypes: WKWebsiteDataStore.allWebsiteDataTypes(), modifiedSince: .distantPast)
        await Task.detached(priority: .userInitiated) { wipeFiles() }.value
        if let id = Bundle.main.bundleIdentifier { UserDefaults.standard.removePersistentDomain(forName: id) }
        UserDefaults(suiteName: PushRegistration.appGroup)?.removePersistentDomain(forName: PushRegistration.appGroup)
        // Like Signal's "Delete all data": the running engine must not write anything back.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.4) { exit(0) }
    }

    nonisolated static func wipeFiles() {
        let fm = FileManager.default
        var roots: [URL] = []
        // Engine state first (identity, friends, chats), the received files last.
        roots += fm.urls(for: .applicationSupportDirectory, in: .userDomainMask)
        roots += fm.urls(for: .cachesDirectory, in: .userDomainMask)
        roots.append(fm.temporaryDirectory)
        if let library = fm.urls(for: .libraryDirectory, in: .userDomainMask).first {
            roots.append(library.appendingPathComponent("WebKit", isDirectory: true))
            roots.append(library.appendingPathComponent("Cookies", isDirectory: true))
        }
        if let group = fm.containerURL(forSecurityApplicationGroupIdentifier: PushRegistration.appGroup) {
            roots.append(group)
            roots.append(group.appendingPathComponent("Library/Caches", isDirectory: true))
        }
        roots += fm.urls(for: .documentDirectory, in: .userDomainMask)
        for root in roots {
            for name in (try? fm.contentsOfDirectory(atPath: root.path)) ?? [] {
                // The group's own Library (Preferences/Caches dirs) belongs to the system;
                // its caches are emptied above, its preferences via removePersistentDomain.
                if root.lastPathComponent != "Caches" && name == "Library" { continue }
                try? fm.removeItem(at: root.appendingPathComponent(name))
            }
        }
    }
}
