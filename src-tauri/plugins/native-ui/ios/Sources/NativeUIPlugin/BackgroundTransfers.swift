import BackgroundTasks
import Foundation
import Network
import UIKit

/// Keeps transfers going when DropBeam leaves the screen (T8).
///
/// - While bytes are moving (live activity forwarded by the plugin's Rust half straight
///   from the engine — the hidden WebView is suspended in the background), leaving the
///   app starts a UIKit background task, ended as soon as nothing is moving or iOS says
///   time is up. That covers the common "switch apps for a moment" case.
/// - iOS 26+: a send the user starts that is big enough to outlast that (≥ 20 MB) also
///   asks for a BGContinuedProcessingTask: the system shows its progress and lets it
///   finish in the background. Progress comes from the same live activity.
/// - Coming back to the foreground, or the network path changing (Wi-Fi ↔ cellular),
///   tells iroh to re-probe its addresses right away instead of waiting for its own
///   monitor (which iOS paused with the app).
@MainActor final class BackgroundTransfers {
    static let shared = BackgroundTransfers()
    struct Activity: Equatable {
        var active = 0
        var sending = 0
        var done: Int64 = 0
        var total: Int64 = 0
    }
    private(set) var activity = Activity()
    private var updatedAt = Date.distantPast
    private var backgroundTask: UIBackgroundTaskIdentifier = .invalid
    private var started = false
    private let pathMonitor = NWPathMonitor()
    private var lastPath: String?
    private var networkNudge: Task<Void, Never>?
    /// The continued-processing task (iOS 26), type-erased so the class stays iOS 17.
    private var continued: AnyObject?
    private var continuedSubmittedAt: Date?
    private var continuedSawActivity = false

    nonisolated static let continuedPrefix = "com.ashtonmiller.dropbeam.transfer."
    nonisolated static let continuedThreshold: Int64 = 20 * 1024 * 1024

    func start() {
        guard !started else { return }
        started = true
        let center = NotificationCenter.default
        center.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { _ in
            Task { @MainActor in BackgroundTransfers.shared.enteredBackground() }
        }
        center.addObserver(forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main) { _ in
            Task { @MainActor in BackgroundTransfers.shared.endBackgroundTask() }
        }
        center.addObserver(forName: UIApplication.didBecomeActiveNotification, object: nil, queue: .main) { _ in
            Task { @MainActor in BackgroundTransfers.shared.nudgeNetwork(after: 0.3) }
        }
        pathMonitor.pathUpdateHandler = { path in
            // Interfaces + status: a real change, not every re-evaluation.
            let key = "\(path.status)|" + path.availableInterfaces.map { "\($0.type)\($0.name)" }.joined(separator: ",")
            Task { @MainActor in
                let me = BackgroundTransfers.shared
                if let last = me.lastPath, last != key { me.nudgeNetwork(after: 1.5) }
                me.lastPath = key
            }
        }
        pathMonitor.start(queue: DispatchQueue(label: "dropbeam.native.path"))
    }

    /// Live activity from the engine (plugin command `transferActivity`).
    func update(_ next: Activity) {
        activity = next
        updatedAt = Date()
        if next.active > 0 {
            if UIApplication.shared.applicationState == .background { beginBackgroundTask() }
        } else {
            endBackgroundTask()
        }
        updateContinued()
    }

    private var moving: Bool { activity.active > 0 && Date().timeIntervalSince(updatedAt) < 30 }

    private func enteredBackground() {
        if moving { beginBackgroundTask() }
    }
    private func beginBackgroundTask() {
        guard backgroundTask == .invalid else { return }
        backgroundTask = UIApplication.shared.beginBackgroundTask(withName: "DropBeam transfer") {
            // Time's up: hand control back (the engine resumes when the app returns).
            Task { @MainActor in BackgroundTransfers.shared.endBackgroundTask() }
        }
    }
    func endBackgroundTask() {
        guard backgroundTask != .invalid else { return }
        UIApplication.shared.endBackgroundTask(backgroundTask)
        backgroundTask = .invalid
    }

    private func nudgeNetwork(after seconds: Double) {
        networkNudge?.cancel()
        networkNudge = Task { @MainActor in
            try? await Task.sleep(for: .seconds(seconds))
            guard !Task.isCancelled, Bridge.shared.webview != nil, Bridge.shared.settings != nil else { return }
            try? await Bridge.shared.action("networkChanged")
            LanDiscovery.shared.refresh()
        }
    }

    // MARK: iOS 26 continued processing

    /// A send the user just started (Send To, Quick Send, chat attachments, share sheet).
    func userStartedSend(paths: [String], to name: String?) {
        guard #available(iOS 26, *) else { return }
        guard continued == nil, continuedSubmittedAt.map({ Date().timeIntervalSince($0) > 60 }) ?? true else { return }
        Task.detached(priority: .utility) {
            let bytes = Self.size(of: paths)
            guard bytes >= Self.continuedThreshold else { return }
            await BackgroundTransfers.shared.submitContinued(count: paths.count, to: name)
        }
    }

    @available(iOS 26, *)
    private func submitContinued(count: Int, to name: String?) {
        guard UIApplication.shared.applicationState == .active else { return }
        let identifier = Self.continuedPrefix + UUID().uuidString.prefix(8)
        // Continued-processing handlers may be registered after launch, one per task.
        let registered = BGTaskScheduler.shared.register(forTaskWithIdentifier: identifier, using: .main) { task in
            MainActor.assumeIsolated { BackgroundTransfers.shared.run(task) }
        }
        guard registered else { NSLog("DropBeam: continued processing not permitted"); return }
        let title = name.map { "Sending to \($0)" } ?? "Sending files"
        let request = BGContinuedProcessingTaskRequest(identifier: identifier, title: title,
                                                       subtitle: count == 1 ? "1 item" : "\(count) items")
        request.strategy = .fail
        do {
            try BGTaskScheduler.shared.submit(request)
            continuedSubmittedAt = Date(); continuedSawActivity = false
        } catch {
            NSLog("DropBeam: continued processing unavailable: %@", error.localizedDescription)
        }
    }

    @available(iOS 26, *)
    private func run(_ task: BGTask) {
        guard let task = task as? BGContinuedProcessingTask else { task.setTaskCompleted(success: false); return }
        continued = task
        task.expirationHandler = {
            Task { @MainActor in
                let me = BackgroundTransfers.shared
                if me.continued === task { me.continued = nil }
            }
        }
        task.progress.totalUnitCount = 1000
        updateContinued()
        // If the send never gets going, close the system UI instead of leaving it stuck.
        DispatchQueue.main.asyncAfter(deadline: .now() + 65) { BackgroundTransfers.shared.updateContinued() }
    }

    private func updateContinued() {
        guard #available(iOS 26, *), let task = continued as? BGContinuedProcessingTask else { return }
        if activity.active > 0 {
            continuedSawActivity = true
            let fraction = activity.total > 0 ? Double(activity.done) / Double(activity.total) : 0
            task.progress.completedUnitCount = Int64(min(999, max(0, fraction * 1000)))
            task.updateTitle(task.title, subtitle: activity.total > 0
                ? "\(ByteCountFormatter.string(fromByteCount: activity.done, countStyle: .file)) of \(ByteCountFormatter.string(fromByteCount: activity.total, countStyle: .file))"
                : "Connecting…")
        } else if continuedSawActivity || Date().timeIntervalSince(continuedSubmittedAt ?? .distantPast) > 60 {
            // Done (or it never started): close the system's progress UI.
            task.progress.completedUnitCount = task.progress.totalUnitCount
            task.setTaskCompleted(success: continuedSawActivity)
            continued = nil
        }
    }

    nonisolated private static func size(of paths: [String]) -> Int64 {
        let fm = FileManager.default
        var total: Int64 = 0
        for path in paths {
            var isDir: ObjCBool = false
            guard fm.fileExists(atPath: path, isDirectory: &isDir) else { continue }
            if !isDir.boolValue { total += (try? fm.attributesOfItem(atPath: path)[.size] as? NSNumber)?.int64Value ?? 0; continue }
            let walker = fm.enumerator(at: URL(fileURLWithPath: path), includingPropertiesForKeys: [.fileSizeKey])
            while let file = walker?.nextObject() as? URL {
                total += Int64((try? file.resourceValues(forKeys: [.fileSizeKey]).fileSize) ?? 0)
                if total >= continuedThreshold { return total }
            }
        }
        return total
    }
}
