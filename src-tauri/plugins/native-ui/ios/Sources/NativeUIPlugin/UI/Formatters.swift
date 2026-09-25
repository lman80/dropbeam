import Foundation

enum Formatters {
    static func bytes(_ value: Double?) -> String {
        let n = max(0, value?.isFinite == true ? (value ?? 0) : 0)
        let units = ["B", "KB", "MB", "GB", "TB"]
        var scaled = n
        var index = 0
        while scaled >= 1000 && index < units.count - 1 { scaled /= 1000; index += 1 }
        return "\(scaled.formatted(.number.precision(.fractionLength(0...1)))) \(units[index])"
    }
    static func speed(_ value: Double?, megabits: Bool = false) -> String {
        guard megabits else { return "\(bytes(value))/s" }
        return "\((max(0, value ?? 0) * 8 / 1_000_000).formatted(.number.precision(.fractionLength(0...1)))) Mbps"
    }
    static func eta(_ seconds: Double?) -> String? {
        guard let seconds, seconds.isFinite, seconds > 0 else { return nil }
        if seconds < 60 { return "Less than a minute left" }
        if seconds < 3600 { return "\(Int(seconds / 60)) min left" }
        return "\(Int(seconds / 3600)) hr \(Int(seconds.truncatingRemainder(dividingBy: 3600) / 60)) min left"
    }
    static func symbol(_ filename: String?) -> String {
        switch (filename ?? "").split(separator: ".").last?.lowercased() {
        case "jpg", "jpeg", "png", "heic", "gif", "webp": return "photo"
        case "mp4", "mov", "mkv", "avi": return "film"
        case "txt", "md", "pdf", "doc", "docx": return "doc.text"
        case "zip", "gz", "7z", "tar", "rar": return "archivebox"
        case "mp3", "wav", "m4a", "flac", "aac": return "music.note"
        default: return "doc"
        }
    }
}

/// A peer label fit for people: a name passes through; network addresses
/// ("192.168.1.40:5", "[fe80::1]:443", "host.local:4000") and raw endpoint ids
/// are hidden (nil) — never show them in primary UI.
func humanPeer(_ value: String?) -> String? {
    guard let raw = value?.trimmingCharacters(in: .whitespacesAndNewlines), !raw.isEmpty else { return nil }
    let lower = raw.lowercased()
    if lower.range(of: #"^\[?[0-9a-f:.]+\]?(:\d+)?$"#, options: .regularExpression) != nil, lower.contains(".") || lower.contains(":") { return nil }
    if lower.range(of: #"^[0-9a-f]{16,}$"#, options: .regularExpression) != nil { return nil }
    if lower.range(of: #"^[a-z0-9.-]+\.(local|lan|home)(:\d+)?$"#, options: .regularExpression) != nil { return nil }
    return raw
}

/// iOS can move an app's data container when the app is updated or reinstalled, so an
/// absolute path saved earlier ("…/Data/Application/<old UUID>/Library/…") goes stale and
/// its photo bubble or history row shows a placeholder. Re-root such a path into the
/// current container when the old one is gone and the file exists there.
enum LocalPaths {
    @MainActor private static var memo: [String: String] = [:]
    @MainActor static func resolve(_ path: String) -> String {
        if let known = memo[path] { return known }
        let isURL = path.hasPrefix("file://")
        let raw = isURL ? (URL(string: path)?.path ?? path) : path
        var result = path
        if !FileManager.default.fileExists(atPath: raw),
           let range = raw.range(of: #"/Data/Application/[0-9A-Fa-f-]{36}/"#, options: .regularExpression) {
            let candidate = (NSHomeDirectory() as NSString).appendingPathComponent(String(raw[range.upperBound...]))
            if FileManager.default.fileExists(atPath: candidate) { result = candidate }
        }
        memo[path] = result
        return result
    }
}
