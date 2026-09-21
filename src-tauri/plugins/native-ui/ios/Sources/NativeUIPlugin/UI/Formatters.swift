import Foundation

enum Formatters {
    static func bytes(_ value: Double?) -> String {
        let n = max(0, value?.isFinite == true ? value! : 0)
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
