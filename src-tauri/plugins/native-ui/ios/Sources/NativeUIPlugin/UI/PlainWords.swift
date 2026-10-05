import SwiftUI
import UIKit

// Everyday words shared by every screen, so iPhone and desktop say the same thing
// (see docs/GLOSSARY.md). Engine names (mirror, two-way, viewer…) never reach people.

/// What a shared folder does, in the owner's words.
enum FolderModeCopy {
    static func title(_ mode: String) -> String {
        switch mode {
        case "mirror": return "Fully synced"
        case "twoWay": return "Add and edit only"
        case "sendOnly": return "Only I can change it"
        case "receiveOnly": return "Only the owner can change it"
        default: return "Shared folder"
        }
    }
    static func detail(_ mode: String) -> String {
        switch mode {
        case "mirror": return "Adding, changing and deleting files happens for everyone. Deleted files can be brought back from Recoverable Files."
        case "twoWay": return "Everyone can add and change files. Deleting a file only removes your own copy."
        case "sendOnly": return "Others get a copy that updates when you change yours."
        case "receiveOnly": return "You get a copy that updates when they change theirs."
        default: return ""
        }
    }
}

/// Editor / Viewer, said plainly.
enum RoleCopy {
    static let editor = "Can add, change and delete files"
    static let viewer = "Can open and copy files, but not change them"
}

/// iOS Settings → DropBeam (permissions: Photos, Camera, Notifications, Local Network).
enum SystemSettings {
    @MainActor static func open() {
        if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
    }
}

enum PlainError {
    /// An error that's fixed by turning a permission on in the Settings app.
    static func needsSettings(_ message: String?) -> Bool {
        guard let m = message?.lowercased() else { return false }
        return m.contains("settings app") || m.contains("settings → apps") || m.contains("allow photos access")
            || m.contains("not allowed") && (m.contains("photo") || m.contains("camera") || m.contains("notification"))
    }
    /// Engine/OS errors that slip through unworded ("os error 61", "deadline has
    /// elapsed", "endpoint…") become one plain sentence; worded ones pass through.
    static func humanize(_ message: String) -> String {
        let m = message.lowercased()
        let network = ["timed out", "timeout", "deadline has elapsed", "connection refused", "connection lost", "connection reset",
                       "network is unreachable", "no route", "unreachable", "failed to connect", "could not connect", "dial"]
        let technical = ["os error", "endpoint", "iroh", "ticket", "quic", "rpc", "errno", "panicked", "::", "invalid type", "json", "serde"]
        if network.contains(where: m.contains) {
            return "DropBeam couldn’t reach the other device. Make sure it’s turned on with DropBeam open, then try again."
        }
        if technical.contains(where: m.contains) { return "Something went wrong. Please try again." }
        return message
    }
}
