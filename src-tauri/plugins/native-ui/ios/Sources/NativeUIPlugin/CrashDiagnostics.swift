import Foundation
import MetricKit

/// Real crash stacks from MetricKit (MXCrashDiagnostic), handed to the existing crash
/// report path: SuperFeedback sends every `*.crash` file in its Crashes folder at the next
/// launch, as a `crash` report — the same switch (Settings → Diagnostics → Share
/// Diagnostics) controls both. The in-process handler only knows the signal; MetricKit
/// adds the exception type/codes, termination reason and the full call-stack tree
/// (binary UUIDs + offsets, symbolicated with the build's dSYM).
final class CrashDiagnostics: NSObject, MXMetricManagerSubscriber {
    static let shared = CrashDiagnostics()
    private let lock = NSLock()
    private var subscribed = false
    private var enabled = false

    func setEnabled(_ on: Bool) {
        lock.lock(); defer { lock.unlock() }
        enabled = on
        if on && !subscribed { MXMetricManager.shared.add(self); subscribed = true }
        else if !on && subscribed { MXMetricManager.shared.remove(self); subscribed = false }
    }

    func didReceive(_ payloads: [MXDiagnosticPayload]) {
        lock.lock(); let on = enabled; lock.unlock()
        guard on else { return }
        for payload in payloads {
            for crash in payload.crashDiagnostics ?? [] { Self.write(Self.report(crash, at: payload.timeStampEnd)) }
        }
    }

    static func report(_ crash: MXCrashDiagnostic, at date: Date) -> String {
        var lines = ["MetricKit crash diagnostic (\(ISO8601DateFormatter().string(from: date)))"]
        let meta = crash.metaData
        lines.append("App \(crash.applicationVersion) build \(meta.applicationBuildVersion) · \(meta.osVersion) · \(meta.deviceType)")
        if let type = crash.exceptionType { lines.append("Exception type \(type)\(crash.exceptionCode.map { " code \($0)" } ?? "")") }
        if let signal = crash.signal { lines.append("Signal \(signal)") }
        if #available(iOS 17.0, *), let reason = crash.exceptionReason {
            lines.append("Reason \(reason.exceptionName): \(reason.composedMessage)")
        }
        if let termination = crash.terminationReason { lines.append("Termination \(termination)") }
        if let region = crash.virtualMemoryRegionInfo { lines.append("VM region \(region)") }
        let tree = String(decoding: crash.callStackTree.jsonRepresentation(), as: UTF8.self)
        lines.append("Call stack tree (JSON):")
        lines.append(String(tree.prefix(6500)))
        return lines.joined(separator: "\n") + "\n"
    }

    /// SuperFeedback's crash folder (Application Support/SuperFeedback/<bundle>/Crashes).
    private static func write(_ text: String) {
        guard let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else { return }
        let dir = base.appendingPathComponent("SuperFeedback", isDirectory: true)
            .appendingPathComponent(Bundle.main.bundleIdentifier ?? ProcessInfo.processInfo.processName, isDirectory: true)
            .appendingPathComponent("Crashes", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        try? Data(text.utf8).write(to: dir.appendingPathComponent("metrickit-\(UUID().uuidString).crash"), options: .atomic)
    }
}
