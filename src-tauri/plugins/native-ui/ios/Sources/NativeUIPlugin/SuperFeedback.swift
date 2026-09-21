// SuperFeedback 2.1.0 — copy this file into an iOS 16+ target.
import Foundation
import SwiftUI
import UIKit
import PhotosUI
import ImageIO
import Darwin
import OSLog
import Network

@MainActor
public enum SuperFeedback {
    public nonisolated static let version = "2.1.0"

    public enum Trigger: Sendable { case draggable, floating, none }
    public enum Position: Sendable {
        case rightCenter, leftCenter, bottomRight, bottomLeft, topRight, topLeft
    }

    public struct Config: Sendable {
        public var backendURL: URL
        public var repo: String
        public var app: String
        public var appKey = ""
        public var trigger: Trigger = .draggable
        public var position: Position? = nil
        public var accent: Color? = nil
        public var attachScreenshot = true
        public var maxImages = 5
        public var captureLogs = true
        public var captureCrashes = true
        public var meta: [String: String] = [:]

        public init(backendURL: URL, repo: String, app: String, appKey: String = "",
                    trigger: Trigger = .draggable, position: Position? = nil,
                    accent: Color? = nil, attachScreenshot: Bool = true, maxImages: Int = 5,
                    captureLogs: Bool = true, captureCrashes: Bool = true,
                    meta: [String: String] = [:]) {
            self.backendURL = backendURL; self.repo = repo; self.app = app; self.appKey = appKey
            self.trigger = trigger; self.position = position; self.accent = accent
            self.attachScreenshot = attachScreenshot; self.maxImages = maxImages
            self.captureLogs = captureLogs; self.captureCrashes = captureCrashes; self.meta = meta
        }
    }

    private static var config: Config?
    private static var scenes: [ObjectIdentifier: SFSceneState] = [:]
    private static var observers: [NSObjectProtocol] = []
    private static var started = false
    private static var context: [String: String] = [:]
    private static var logs: [SFLogLine] = []
    private static var logsWriteTask: Task<Void, Never>?
    private static var initializedAt = ProcessInfo.processInfo.systemUptime
    private static let sessionId = UUID().uuidString
    private static let networkMonitor = NWPathMonitor()
    private static var network = "unknown"
    private static let delivery = SFDelivery()
    fileprivate static let enabledChanged = Notification.Name("superfeedback.enabledChanged")

    public static func configure(_ config: Config) {
        if self.config == nil { initializedAt = ProcessInfo.processInfo.systemUptime }
        self.config = config
        for state in scenes.values { state.config = config }
        SFCrashCapture.setEnabled(config.captureCrashes)
        if !config.captureLogs {
            logs = []
            logsWriteTask?.cancel(); logsWriteTask = nil
        }
    }

    /// Safe from App.init(), before UIKit has connected any scenes. Idempotent.
    public static func start() {
        guard let config, !started else { return }
        started = true
        // Consume the previous session's ring before this session can write or crash.
        let previousLogs = config.captureLogs ? SFStorage.readLogs() : []
        if let url = SFStorage.directory()?.appendingPathComponent("diagnostics.log") {
            try? FileManager.default.removeItem(at: url)
        }
        if !logs.isEmpty { scheduleLogsWrite() }
        networkMonitor.pathUpdateHandler = { path in
            let status = path.status != .satisfied ? "offline"
                : path.usesInterfaceType(.wifi) ? "wifi"
                : path.usesInterfaceType(.cellular) ? "cellular" : "online"
            Task { @MainActor in network = status }
        }
        networkMonitor.start(queue: DispatchQueue(label: "superfeedback.network", qos: .utility))
        for name in [UIScene.willConnectNotification, UIScene.didActivateNotification,
                     UIScene.didDisconnectNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) {
                notification in
                guard let scene = notification.object as? UIWindowScene else { return }
                let notificationName = notification.name
                MainActor.assumeIsolated {
                    if notificationName == UIScene.didDisconnectNotification {
                        let state = scenes.removeValue(forKey: ObjectIdentifier(scene))
                        state?.tearDown()
                    } else { install(in: scene) }
                }
            })
        }
        for name in [UIApplication.didEnterBackgroundNotification, UIScene.didEnterBackgroundNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { _ in
                MainActor.assumeIsolated { persistLogs() }
            })
        }
        for scene in UIApplication.shared.connectedScenes.compactMap({ $0 as? UIWindowScene }) {
            install(in: scene)
        }
        let crashMeta = metadata(in: activeScene(), config: config)
        Task {
            var readyMeta = crashMeta
            let resources = await Task.detached(priority: .utility) { SFDiagnostics.resourceMetadata() }.value
            readyMeta.merge(resources) { _, new in new }
            await delivery.start(config: config, crashMeta: readyMeta, previousLogs: previousLogs)
        }
    }

    public static func present() {
        guard config != nil else { return }
        start()
        guard let scene = activeScene() else { return }
        install(in: scene)
        scenes[ObjectIdentifier(scene)]?.present()
    }

    /// Presents in the caller's window scene; nil uses the active-scene fallback.
    public static func present(in scene: UIWindowScene?) {
        guard let scene else { present(); return }
        guard config != nil else { return }
        start()
        install(in: scene)
        scenes[ObjectIdentifier(scene)]?.present()
    }

    public static func dismiss() {
        if let scene = activeScene(), let state = scenes[ObjectIdentifier(scene)], state.isPresented {
            state.dismiss()
        } else { scenes.values.first(where: { $0.isPresented })?.dismiss() }
    }

    public static func log(_ message: String, level: String = "info") {
        guard config?.captureLogs == true else { return }
        let now = Date()
        logs.append(SFLogLine(date: now, line: SFDiagnostics.format(
            date: now, level: level, source: "app", text: message)))
        logs = SFDiagnostics.bounded(logs)
        scheduleLogsWrite()
    }

    public static func setContext(_ values: [String: String]) {
        context.merge(values) { _, new in new }
    }

    public static var isEnabled: Bool {
        UserDefaults.standard.object(forKey: "superfeedback.enabled") as? Bool ?? true
    }

    public static func setEnabled(_ on: Bool) {
        UserDefaults.standard.set(on, forKey: "superfeedback.enabled")
        for state in scenes.values {
            state.enabled = on
            if !on { state.buttonFrame = .zero }
        }
        NotificationCenter.default.post(name: enabledChanged, object: nil)
    }

    private static func install(in scene: UIWindowScene) {
        let id = ObjectIdentifier(scene)
        guard scenes[id] == nil, let config else { return }
        let state = SFSceneState(scene: scene, config: config)
        scenes[id] = state
        let window = SFOverlayWindow(windowScene: scene)
        window.state = state
        window.windowLevel = .init(rawValue: UIWindow.Level.alert.rawValue - 1)
        window.backgroundColor = .clear
        window.isOpaque = false
        let host = UIHostingController(rootView: SFOverlayView(state: state))
        host.view.backgroundColor = .clear
        host.view.isOpaque = false
        window.rootViewController = host
        state.window = window
        window.isHidden = false // Do not steal the app's key window to show the trigger.
    }

    private static func activeScene() -> UIWindowScene? {
        let connected = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }
        let active = connected.filter { $0.activationState == .foregroundActive }
        return active.first(where: { $0.windows.contains(where: \.isKeyWindow) })
            ?? connected.first(where: { $0.activationState == .foregroundInactive && $0.windows.contains(where: \.isKeyWindow) })
            ?? active.first
    }

    private static func scheduleLogsWrite() {
        // Coalesce bursts without postponing the write on every new breadcrumb.
        guard started, config?.captureLogs == true, logsWriteTask == nil else { return }
        logsWriteTask = Task {
            try? await Task.sleep(nanoseconds: 2_000_000_000)
            guard !Task.isCancelled else { return }
            persistLogs()
        }
    }

    private static func persistLogs() {
        logsWriteTask?.cancel(); logsWriteTask = nil
        guard config?.captureLogs == true else { return }
        // Synchronous, bounded write: background suspension must not cancel persistence.
        if let url = SFStorage.directory()?.appendingPathComponent("diagnostics.log") {
            try? Data(logs.map(\.line).joined(separator: "\n").utf8).write(to: url, options: .atomic)
        }
    }

    fileprivate static func submit(from state: SFSceneState) {
        let message = state.message.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !message.isEmpty, state.isPresented else { return }
        let config = state.config
        let breadcrumbs = logs
        let sentAt = Date()
        let screenshotStatus = state.screenshotURL == nil
            ? "capture failed: \(state.screenshotFailure ?? "unavailable")"
            : !state.attachScreenshot ? "declined"
            : state.annotatedScreenshot != nil
                ? "attached (annotated, \(state.markupShapes.count) shape\(state.markupShapes.count == 1 ? "" : "s"))" : "attached"
        let report = SFReport(type: state.type, message: message,
                              screenshot: state.attachScreenshot ? state.screenshotURL : nil,
                              images: state.attachments.map(\.dataURL),
                              logs: nil,
                              meta: metadata(in: state.scene, config: config, screenshot: screenshotStatus))
        state.dismiss()
        Task { [weak state] in
            // Give a backgrounded app time to persist and finish its request.
            let background = SFBackgroundLease()
            let ready = await Task.detached(priority: .utility) {
                var ready = report
                ready.meta.merge(SFDiagnostics.resourceMetadata()) { _, new in new }
                if config.captureLogs {
                    ready.logs = SFDiagnostics.collect(breadcrumbs: breadcrumbs, until: sentAt)
                }
                return ready
            }.value
            let result = await delivery.submit(ready, config: config)
            background.end()
            state?.showToast(result.ok ? "Thanks! Feedback sent ✓" : "Couldn't send — will retry next launch")
        }
    }

    fileprivate static func metadata(in scene: UIWindowScene?, config: Config,
                                     screenshot: String = "capture failed: unavailable for recovered crash") -> [String: String] {
        let info = Bundle.main.infoDictionary ?? [:]
        let short = info["CFBundleShortVersionString"] as? String ?? "?"
        let build = info["CFBundleVersion"] as? String
        let window = scene?.windows.first(where: { $0.isKeyWindow && !($0 is SFOverlayWindow) })
            ?? scene?.windows.first(where: { !($0 is SFOverlayWindow) && !$0.isHidden })
        let size = window?.bounds.size
            ?? scene?.coordinateSpace.bounds.size ?? .zero
        var values = ["platform": "\(UIDevice.current.systemName) \(UIDevice.current.systemVersion)",
                      "os": "ios", "locale": Locale.current.identifier,
                      "appVersion": build.map { "\(short) (\($0))" } ?? short,
                      "device": hardwareModel(), "viewport": "\(Int(size.width))x\(Int(size.height))",
                      "widget": "ios/\(version)"]
        values.merge(config.meta) { _, new in new }
        values.merge(context) { _, new in new }
        if let route = context["route"] { values["url"] = route }
        values.removeValue(forKey: "route")
        let process = ProcessInfo.processInfo
        let screen = scene?.screen
        let screenSize = screen?.bounds.size ?? size
        let scale = screen?.scale ?? 1
        let traits = window?.traitCollection ?? UITraitCollection.current
        let thermal: String
        switch process.thermalState {
        case .nominal: thermal = "nominal"
        case .fair: thermal = "fair"
        case .serious: thermal = "serious"
        case .critical: thermal = "critical"
        @unknown default: thermal = "unknown"
        }
        let orientation: String
        switch scene?.interfaceOrientation {
        case .portrait: orientation = "portrait"
        case .portraitUpsideDown: orientation = "portraitUpsideDown"
        case .landscapeLeft: orientation = "landscapeLeft"
        case .landscapeRight: orientation = "landscapeRight"
        default: orientation = "unknown"
        }
        values.merge([
            "uptime": String(max(0, process.systemUptime - initializedAt)),
            "timezone": TimeZone.current.identifier,
            "colorScheme": traits.userInterfaceStyle == .dark ? "dark" : "light",
            "screenSize": "\(Int(screenSize.width))x\(Int(screenSize.height)) @\(scale.formatted())x",
            "network": network, "lowPowerMode": String(process.isLowPowerModeEnabled),
            "thermalState": thermal, "orientation": orientation,
            "reduceMotion": String(UIAccessibility.isReduceMotionEnabled),
            "build": build ?? "?", "sessionId": sessionId, "screenshot": screenshot
        ]) { _, new in new }
        return values
    }

    private static func hardwareModel() -> String {
        if let simulator = ProcessInfo.processInfo.environment["SIMULATOR_MODEL_IDENTIFIER"] {
            return simulator + " (Simulator)"
        }
        var system = utsname()
        uname(&system)
        return withUnsafeBytes(of: &system.machine) { String(decoding: $0.prefix { $0 != 0 }, as: UTF8.self) }
    }
}

@MainActor
public struct SuperFeedbackSettingsToggle: View {
    @State private var enabled = SuperFeedback.isEnabled
    public init() {}
    public var body: some View {
        VStack(alignment: .leading, spacing: 5) {
            Toggle("Show feedback button", isOn: Binding(get: { enabled }, set: {
                enabled = $0; SuperFeedback.setEnabled($0)
            }))
            Text("Turn the floating feedback button on or off. You can still send feedback from here.")
                .font(.caption).foregroundStyle(.secondary)
        }
        .onAppear { enabled = SuperFeedback.isEnabled }
        .onReceive(NotificationCenter.default.publisher(for: SuperFeedback.enabledChanged)) { _ in
            enabled = SuperFeedback.isEnabled
        }
    }
}

@MainActor
public struct SuperFeedbackSettingsRow: View {
    @State private var scene: UIWindowScene?
    public init() {}
    public var body: some View {
        Button(action: { SuperFeedback.present(in: scene) }) {
            Label("Send feedback…", systemImage: "bubble.left.and.bubble.right")
        }
        .background(SFSceneReader(scene: $scene).allowsHitTesting(false))
    }
}

@MainActor
private struct SFSceneReader: UIViewRepresentable {
    @Binding var scene: UIWindowScene?

    func makeUIView(context: Context) -> SceneView { SceneView() }
    func updateUIView(_ view: SceneView, context: Context) {
        view.onSceneChange = { if scene !== $0 { scene = $0 } }
        view.resolveScene()
    }

    final class SceneView: UIView {
        var onSceneChange: ((UIWindowScene?) -> Void)?
        override func didMoveToWindow() {
            super.didMoveToWindow()
            resolveScene()
        }
        func resolveScene() {
            // Publish outside SwiftUI's update/layout pass, using the current window.
            DispatchQueue.main.async { [weak self] in
                guard let self else { return }
                onSceneChange?(window?.windowScene)
            }
        }
    }
}

// MARK: - Per-scene presentation and touch routing

@MainActor
private final class SFBackgroundLease {
    private var identifier: UIBackgroundTaskIdentifier = .invalid
    init() {
        identifier = UIApplication.shared.beginBackgroundTask(withName: "SuperFeedback upload") { [weak self] in
            MainActor.assumeIsolated { self?.end() }
        }
    }
    func end() {
        guard identifier != .invalid else { return }
        UIApplication.shared.endBackgroundTask(identifier)
        identifier = .invalid
    }
}

@MainActor
fileprivate final class SFSceneState: ObservableObject {
    weak var scene: UIWindowScene?
    var window: SFOverlayWindow?
    weak var previousKeyWindow: UIWindow?
    @Published var config: SuperFeedback.Config
    @Published var enabled = SuperFeedback.isEnabled
    @Published var isPresented = false
    @Published var toast: String?
    @Published var message = ""
    @Published var type = "bug"
    @Published var attachScreenshot = true
    @Published var attachments: [SFAttachment] = []
    @Published var safeAreaInsets: UIEdgeInsets = .zero
    /// Markup shapes in normalised image coordinates; the base image is never rasterised.
    @Published var markupShapes: [SFShape] = []
    /// Non-nil once shapes have been composited; drives the thumbnail and the annotated status.
    @Published var annotatedScreenshot: UIImage?
    /// The clean capture, kept for re-editing and for reverting when all shapes are removed.
    var screenshot: UIImage?
    var screenshotURL: String?
    var screenshotCleanURL: String?
    var screenshotFailure: String?
    var buttonFrame: CGRect = .zero
    var toastFrame: CGRect = .zero
    var draftID = UUID()
    var toastTask: Task<Void, Never>?
    @Published var sheetClosing = false
    private var dismissalFallback: DispatchWorkItem?

    init(scene: UIWindowScene, config: SuperFeedback.Config) {
        self.scene = scene; self.config = config
    }

    var accent: Color { config.accent ?? Color(red: 109 / 255, green: 94 / 255, blue: 252 / 255) }
    var showsButton: Bool { enabled && config.trigger != .none && !isPresented && !sheetClosing }

    func present() {
        guard !isPresented, !sheetClosing, let scene else { return }
        clearDraft()
        switch SFImages.capture(scene: scene) {
        case .success(let capture):
            screenshot = capture.image
            screenshotURL = capture.url; screenshotCleanURL = capture.url
        case .failure(let failure): screenshotFailure = failure.rawValue
        }
        attachScreenshot = config.attachScreenshot
        previousKeyWindow = scene.windows.first { $0.isKeyWindow && !($0 is SFOverlayWindow) }
            ?? scene.windows.first { !($0 is SFOverlayWindow) && !$0.isHidden && $0.windowLevel == .normal }
        isPresented = true
        window?.makeKey()
    }

    func dismiss() {
        guard isPresented else { return }
        isPresented = false
        beginDismissal()
    }

    func beginDismissal() {
        guard !isPresented else { return }
        sheetClosing = true
        window?.endEditing(true)
        dismissalFallback?.cancel()
        let fallback = DispatchWorkItem { [weak self] in
            guard let self, !self.isPresented else { return }
            self.didDismiss()
        }
        dismissalFallback = fallback
        // onDismiss normally completes cleanup. The timer covers interrupted
        // UIKit transitions/backgrounding so sheetClosing can never latch shut.
        DispatchQueue.main.asyncAfter(deadline: .now() + 1.2, execute: fallback)
    }

    func didDismiss() {
        dismissalFallback?.cancel(); dismissalFallback = nil
        isPresented = false
        sheetClosing = false
        if window?.isKeyWindow == true { previousKeyWindow?.makeKey() }
        previousKeyWindow = nil
        clearDraft()
        objectWillChange.send()
    }

    private func clearDraft() {
        draftID = UUID(); message = ""; type = "bug"; attachments = []
        screenshot = nil; screenshotURL = nil; screenshotCleanURL = nil; screenshotFailure = nil
        markupShapes = []; annotatedScreenshot = nil
    }

    /// Commits the editor's shapes: composite once, here, and send the annotated PNG instead of
    /// the clean one. The clean capture and its data URL stay put so re-editing starts from them.
    func applyMarkup(_ shapes: [SFShape]) {
        markupShapes = shapes
        guard !shapes.isEmpty, let base = screenshot,
              let composited = SFMarkup.render(base, shapes: shapes),
              case .success(let encoded) = SFImages.encodePNG(composited) else {
            annotatedScreenshot = nil
            screenshotURL = screenshotCleanURL
            return
        }
        annotatedScreenshot = encoded.image
        screenshotURL = encoded.url
    }

    func showToast(_ text: String) {
        toastTask?.cancel()
        toast = text
        UIAccessibility.post(notification: .announcement, argument: text)
        toastTask = Task { [weak self] in
            try? await Task.sleep(nanoseconds: 2_600_000_000)
            guard !Task.isCancelled else { return }
            self?.toast = nil
            self?.toastFrame = .zero
        }
    }

    func tearDown() {
        toastTask?.cancel()
        window?.isHidden = true
        window?.rootViewController = nil
        window = nil
    }
}

@MainActor
fileprivate final class SFOverlayWindow: UIWindow {
    weak var state: SFSceneState?
    override func layoutSubviews() {
        super.layoutSubviews()
        let insets = safeAreaInsets
        // Publishing from inside UIKit's layout pass can invalidate SwiftUI mid-update.
        DispatchQueue.main.async { [weak self] in
            guard let state = self?.state, state.safeAreaInsets != insets else { return }
            state.safeAreaInsets = insets
        }
    }
    override func hitTest(_ point: CGPoint, with event: UIEvent?) -> UIView? {
        guard let state else { return nil }
        // The toast never owns a touch, even if it overlaps a button or a sheet.
        if state.toast != nil && state.toastFrame.contains(point) { return nil }
        // Guarantee: transparent window space NEVER owns touches, including
        // dismissal. Only the visible panel (or measured trigger) can hit-test.
        // A stale presented controller/sheetClosing flag cannot cover friend rows.
        if state.isPresented, var panel = rootViewController?.presentedViewController {
            while let next = panel.presentedViewController { panel = next }
            guard !panel.isBeingDismissed, let view = panel.viewIfLoaded, !view.isHidden else { return nil }
            let frame = view.convert(view.bounds, to: self)
            guard frame.contains(point) else { return nil }
            // iOS 26 can route sheets through a separate floating container;
            // UIWindow's hit is then not a descendant of the hosting view.
            // Hit-test the actual panel locally, still bounded by its frame.
            return view.hitTest(view.convert(point, from: self), with: event)
        }
        let frame = state.buttonFrame
        // AshTranslate's fail-open guard: a full-screen measurement must never freeze the app.
        guard state.showsButton, frame.width > 0, frame.height > 0,
              frame.width <= 120, frame.height <= 120, frame.contains(point) else { return nil }
        return super.hitTest(point, with: event)
    }
}

private struct SFFrameKey: PreferenceKey {
    static let defaultValue: CGRect = .zero
    static func reduce(value: inout CGRect, nextValue: () -> CGRect) { value = nextValue() }
}

private extension View {
    @MainActor @ViewBuilder
    func sfOnChange<Value: Equatable>(of value: Value, perform: @escaping (Value) -> Void) -> some View {
        if #available(iOS 17, *) {
            onChange(of: value) { _, new in perform(new) }
        } else {
            onChange(of: value, perform: perform)
        }
    }
}

@MainActor
private struct SFOverlayView: View {
    @ObservedObject var state: SFSceneState
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    @Environment(\.displayScale) private var displayScale
    @State private var resting: SFButtonPosition?
    @State private var dragOrigin: CGPoint?
    @State private var draggedCenter: CGPoint?
    @State private var dragging = false
    @GestureState private var pressed = false
    private let space = "superfeedback.overlay"

    var body: some View {
        GeometryReader { geometry in
            ZStack(alignment: .top) {
                Color.clear
                if state.showsButton {
                    trigger(geometry)
                }
                if let toast = state.toast {
                    Text(toast)
                        .font(.subheadline.weight(.medium)).multilineTextAlignment(.center)
                        .padding(.horizontal, 18).padding(.vertical, 12)
                        .background(Color(uiColor: .secondarySystemBackground), in: Capsule())
                        .shadow(color: .black.opacity(0.15), radius: 10, y: 3)
                        .background(GeometryReader { proxy in
                            Color.clear.preference(key: SFFrameKey.self, value: proxy.frame(in: .named(space)))
                        })
                        .onPreferenceChange(SFFrameKey.self) { frame in state.toastFrame = frame }
                        .padding(.horizontal, 20).padding(.top, state.safeAreaInsets.top + 12)
                        .allowsHitTesting(false).accessibilityHidden(true)
                }
            }
            .frame(width: geometry.size.width, height: geometry.size.height)
            .coordinateSpace(name: space)
            .onAppear { resting = SFButtonPosition.restore() }
            .sfOnChange(of: geometry.size) { _ in resetDrag() }
        }
        .ignoresSafeArea(.container)
        .tint(state.accent)
        .sfOnChange(of: state.isPresented) { presented in
            if !presented { state.beginDismissal() }
        }
        .sheet(isPresented: $state.isPresented, onDismiss: { state.didDismiss() }) {
            SFPanel(state: state)
                .presentationDetents([.medium, .large])
                .presentationDragIndicator(.visible)
                .tint(state.accent)
        }
    }

    private func trigger(_ geometry: GeometryProxy) -> some View {
        let bounds = movementBounds(geometry)
        let configuredPosition = state.config.position
            ?? (state.config.trigger == .floating ? .bottomRight : .rightCenter)
        let position = state.config.trigger == .draggable
            ? (resting ?? SFButtonPosition(position: configuredPosition))
            : SFButtonPosition(position: configuredPosition)
        let center = draggedCenter.map { clamp($0, to: bounds) } ?? position.center(in: bounds)
        return Image(systemName: "bubble.left.and.bubble.right.fill")
            .font(.system(size: 17, weight: .semibold)).foregroundStyle(Color.primary.opacity(0.8))
            .frame(width: 40, height: 40)
            .background(.ultraThinMaterial, in: Circle())
            .overlay(Circle().strokeBorder(Color.primary.opacity(0.12), lineWidth: 1 / max(1, displayScale)))
            .shadow(color: .black.opacity(0.12), radius: 6, y: 3)
            .contentShape(Circle())
            .opacity(pressed ? 1 : 0.62)
            .scaleEffect(dragging && state.config.trigger == .draggable ? 1.1 : 1)
            // Measure before .position(), whose layout frame is the entire overlay.
            .background(GeometryReader { proxy in
                Color.clear.preference(key: SFFrameKey.self, value: proxy.frame(in: .named(space)))
            })
            .onPreferenceChange(SFFrameKey.self) { frame in state.buttonFrame = frame }
            .gesture(DragGesture(minimumDistance: 0, coordinateSpace: .named(space))
                .updating($pressed) { _, pressed, _ in pressed = true }
                .onChanged { value in
                    if dragOrigin == nil { dragOrigin = center }
                    if hypot(value.translation.width, value.translation.height) > 6 { dragging = true }
                    if dragging && state.config.trigger == .draggable, let origin = dragOrigin {
                        draggedCenter = clamp(CGPoint(x: origin.x + value.translation.width,
                                                     y: origin.y + value.translation.height), to: bounds)
                    }
                }
                .onEnded { value in
                    let moved = dragging || hypot(value.translation.width, value.translation.height) > 6
                    if moved && state.config.trigger == .draggable {
                        let origin = dragOrigin ?? center
                        let landed = clamp(CGPoint(x: origin.x + value.translation.width,
                                                   y: origin.y + value.translation.height), to: bounds)
                        let spot = SFButtonPosition(side: landed.x < bounds.midX ? "left" : "right",
                                                    y: bounds.height > 0 ? (landed.y - bounds.minY) / bounds.height : 0.5)
                        spot.persist()
                        withAnimation(reduceMotion ? nil : .spring(response: 0.3, dampingFraction: 0.85)) {
                            resting = spot; resetDrag()
                        }
                    } else {
                        resetDrag()
                        if !moved { state.present() }
                    }
                })
            .position(center)
            .sfOnChange(of: pressed) { value in
                if !value {
                    // Let onEnded use the latched movement threshold before clearing a cancelled gesture.
                    DispatchQueue.main.async { if !pressed { resetDrag() } }
                }
            }
            .accessibilityLabel("Send feedback")
            .accessibilityHint(state.config.trigger == .draggable ? "Captures the app and opens a feedback form. Drag to move the button." : "Captures the app and opens a feedback form.")
            .accessibilityAddTraits(.isButton)
            .accessibilityAction { state.present() }
    }

    private func resetDrag() { dragOrigin = nil; draggedCenter = nil; dragging = false }
    private func movementBounds(_ geometry: GeometryProxy) -> CGRect {
        let insets = state.safeAreaInsets
        // 20pt radius plus the existing 12pt edge gap.
        let left = min(geometry.size.width / 2, insets.left + 32)
        let top = min(geometry.size.height / 2, insets.top + 32)
        return CGRect(x: left, y: top,
                      width: max(0, geometry.size.width - insets.right - 32 - left),
                      height: max(0, geometry.size.height - insets.bottom - 32 - top))
    }
    private func clamp(_ point: CGPoint, to rect: CGRect) -> CGPoint {
        CGPoint(x: min(max(point.x, rect.minX), rect.maxX), y: min(max(point.y, rect.minY), rect.maxY))
    }
}

private struct SFButtonPosition: Sendable {
    var side: String
    var y: CGFloat
    init(side: String, y: CGFloat) { self.side = side; self.y = y }
    init(position: SuperFeedback.Position) {
        switch position {
        case .leftCenter: self.init(side: "left", y: 0.5)
        case .rightCenter: self.init(side: "right", y: 0.5)
        case .bottomLeft: self.init(side: "left", y: 1)
        case .bottomRight: self.init(side: "right", y: 1)
        case .topLeft: self.init(side: "left", y: 0)
        case .topRight: self.init(side: "right", y: 0)
        }
    }
    func center(in rect: CGRect) -> CGPoint {
        CGPoint(x: side == "left" ? rect.minX : rect.maxX, y: rect.minY + min(max(y, 0), 1) * rect.height)
    }
    func persist() {
        UserDefaults.standard.set(["side": side, "y": Double(y)], forKey: "superfeedback.buttonPosition")
    }
    static func restore() -> Self? {
        guard let saved = UserDefaults.standard.dictionary(forKey: "superfeedback.buttonPosition"),
              let side = saved["side"] as? String, ["left", "right"].contains(side),
              let y = saved["y"] as? Double, y.isFinite else { return nil }
        return Self(side: side, y: min(max(y, 0), 1))
    }
}

// MARK: - Composer and image processing

private struct SFAttachment: Identifiable {
    let id = UUID()
    let image: UIImage
    let dataURL: String
}

@MainActor
private struct SFPanel: View {
    @ObservedObject var state: SFSceneState
    @State private var selection: [PhotosPickerItem] = []
    @State private var loading = false
    @State private var photoTask: Task<Void, Never>?
    @State private var markingUp = false

    private var markupTitle: String { state.markupShapes.isEmpty ? "Mark up" : "Edit markup" }

    var body: some View {
        NavigationStack {
            Form {
                Section {
                    Text(state.config.app).font(.subheadline).foregroundStyle(.secondary)
                    Picker("Type", selection: $state.type) {
                        Text("🐞 Bug").tag("bug")
                        Text("✨ Idea").tag("feature")
                        Text("💬 Other").tag("other")
                    }.pickerStyle(.segmented)
                    ZStack(alignment: .topLeading) {
                        TextEditor(text: $state.message).frame(minHeight: 110)
                            .accessibilityLabel("Feedback message")
                        if state.message.isEmpty {
                            Text("What went wrong, or what would you like?")
                                .foregroundStyle(.secondary).padding(.top, 8).padding(.leading, 5)
                                .allowsHitTesting(false)
                        }
                    }
                    Text(state.config.captureLogs
                         ? "Includes a screenshot and recent app logs" : "Includes a screenshot")
                        .font(.caption).foregroundStyle(.secondary)
                }
                if let screenshot = state.screenshot {
                    Section {
                        HStack {
                            Button { markingUp = true } label: {
                                Image(uiImage: state.annotatedScreenshot ?? screenshot)
                                    .resizable().scaledToFit()
                                    .frame(width: 56, height: 64)
                                    .clipShape(RoundedRectangle(cornerRadius: 6))
                                    .opacity(state.attachScreenshot ? 1 : 0.4)
                            }
                            .buttonStyle(.borderless).accessibilityLabel(markupTitle)
                            Toggle("Attach screenshot", isOn: $state.attachScreenshot)
                        }
                        Button { markingUp = true } label: {
                            Label(markupTitle, systemImage: "pencil.tip.crop.circle")
                        }
                        .buttonStyle(.borderless)
                    }
                }
                if state.config.maxImages > 0 {
                    Section {
                        if state.attachments.count < state.config.maxImages {
                            PhotosPicker(selection: $selection,
                                         maxSelectionCount: max(1, state.config.maxImages - state.attachments.count),
                                         matching: .images) {
                                Label("Add image", systemImage: "photo.badge.plus")
                            }.disabled(loading)
                        }
                        if loading { ProgressView("Loading images…") }
                        if !state.attachments.isEmpty {
                            ScrollView(.horizontal) {
                                HStack {
                                    ForEach(state.attachments) { attachment in
                                        VStack {
                                            Image(uiImage: attachment.image).resizable().scaledToFill()
                                                .frame(width: 64, height: 64).clipped()
                                                .clipShape(RoundedRectangle(cornerRadius: 6))
                                            Button("Remove", role: .destructive) {
                                                state.attachments.removeAll { $0.id == attachment.id }
                                            }.font(.caption).buttonStyle(.borderless)
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            .navigationTitle("Send feedback").navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Cancel") { state.dismiss() } }
                ToolbarItem(placement: .confirmationAction) {
                    Button("Send") { SuperFeedback.submit(from: state) }
                        .disabled(state.message.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty || loading)
                }
            }
            .sfOnChange(of: selection) { items in load(items) }
            .onDisappear { photoTask?.cancel() }
        }
        .fullScreenCover(isPresented: $markingUp) {
            if let screenshot = state.screenshot {
                SFMarkupEditor(image: screenshot, shapes: state.markupShapes,
                               cancel: { markingUp = false },
                               commit: { shapes in state.applyMarkup(shapes); markingUp = false })
            }
        }
    }

    private func load(_ items: [PhotosPickerItem]) {
        guard !items.isEmpty else { return }
        photoTask?.cancel()
        loading = true
        let draftID = state.draftID
        let remaining = max(0, state.config.maxImages - state.attachments.count)
        photoTask = Task {
            defer { loading = false }
            for item in items.prefix(remaining) {
                guard !Task.isCancelled, state.isPresented, state.draftID == draftID else { return }
                guard let data = try? await item.loadTransferable(type: Data.self) else { continue }
                guard !Task.isCancelled, state.isPresented, state.draftID == draftID else { return }
                if let attachment = SFImages.attachment(data) { state.attachments.append(attachment) }
            }
            selection = []
        }
    }
}

@MainActor
private enum SFImages {
    enum CaptureFailure: String, Error {
        case noWindows = "no visible app windows"
        case invalidBounds = "invalid scene bounds"
        case encoding = "PNG encoding failed"
        case tooLarge = "PNG exceeds 2 MB after downscaling"
    }
    static func capture(scene: UIWindowScene) -> Result<(image: UIImage, url: String), CaptureFailure> {
        let windows = scene.windows.filter { !($0 is SFOverlayWindow) && !$0.isHidden && $0.alpha > 0 }
            .sorted { $0.windowLevel.rawValue < $1.windowLevel.rawValue }
        let bounds = scene.coordinateSpace.bounds
        guard !windows.isEmpty else { return .failure(.noWindows) }
        guard bounds.width > 0, bounds.height > 0 else { return .failure(.invalidBounds) }
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1; format.opaque = true
        let image = UIGraphicsImageRenderer(size: bounds.size, format: format).image { context in
            UIColor.systemBackground.setFill(); context.fill(CGRect(origin: .zero, size: bounds.size))
            for window in windows {
                let rect = window.convert(window.bounds, to: scene.coordinateSpace)
                context.cgContext.saveGState()
                context.cgContext.translateBy(x: rect.minX - bounds.minX, y: rect.minY - bounds.minY)
                context.cgContext.scaleBy(x: rect.width / max(1, window.bounds.width),
                                         y: rect.height / max(1, window.bounds.height))
                if !window.drawHierarchy(in: window.bounds, afterScreenUpdates: false) {
                    window.layer.render(in: context.cgContext)
                }
                context.cgContext.restoreGState()
            }
        }
        return encodePNG(image)
    }

    /// Initial encode plus at most five 0.75 downscales. Never emit an oversized PNG.
    /// Shared by capture and by the markup composite so both obey the same ceiling.
    static func encodePNG(_ original: UIImage) -> Result<(image: UIImage, url: String), CaptureFailure> {
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1; format.opaque = true
        var image = original
        for attempt in 0...5 {
            guard let png = image.pngData() else { return .failure(.encoding) }
            if png.count <= 2 * 1024 * 1024 {
                return .success((image, "data:image/png;base64," + png.base64EncodedString()))
            }
            if attempt < 5 {
                let size = CGSize(width: max(1, floor(image.size.width * 0.75)),
                                  height: max(1, floor(image.size.height * 0.75)))
                let current = image
                image = UIGraphicsImageRenderer(size: size, format: format).image { _ in
                    current.draw(in: CGRect(origin: .zero, size: size))
                }
            }
        }
        return .failure(.tooLarge)
    }

    static func attachment(_ data: Data) -> SFAttachment? {
        // ImageIO downsamples during decoding, avoiding a full-resolution photo allocation.
        guard let source = CGImageSourceCreateWithData(data as CFData, nil),
              let cgImage = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceThumbnailMaxPixelSize: 1600
              ] as CFDictionary) else { return nil }
        let image = UIImage(cgImage: cgImage)
        guard let jpeg = image.jpegData(compressionQuality: 0.85) else { return nil }
        return SFAttachment(image: image, dataURL: "data:image/jpeg;base64," + jpeg.base64EncodedString())
    }
}

// MARK: - Screenshot markup

private enum SFMarkupTool: String, Sendable, CaseIterable, Identifiable {
    case pen, circle, arrow, rectangle
    var id: String { rawValue }
    var title: String {
        switch self {
        case .pen: return "Pen"
        case .circle: return "Circle"
        case .arrow: return "Arrow"
        case .rectangle: return "Rectangle"
        }
    }
    var symbol: String {
        switch self {
        case .pen: return "scribble"
        case .circle: return "circle"
        case .arrow: return "arrow.up.right"
        case .rectangle: return "rectangle"
        }
    }
}

/// Resolution-independent: points are normalised (0…1) image coordinates, so the same shape
/// renders identically in the editor's fitted frame and in the native-size export.
private struct SFShape: Identifiable, Sendable, Equatable {
    let id: UUID
    var tool: SFMarkupTool
    var color: String
    var points: [CGPoint]

    init(id: UUID = UUID(), tool: SFMarkupTool, color: String, points: [CGPoint]) {
        self.id = id; self.tool = tool; self.color = color; self.points = points
    }
}

private enum SFMarkup {
    static let colors = ["#ff3b30", "#ffcc00", "#0a84ff"]

    /// Stroke width in *image* pixels, so markup reads the same on every device.
    static func lineWidth(imageWidth: CGFloat) -> CGFloat { max(3, imageWidth / 300) }

    static func fit(_ image: CGSize, in bounds: CGSize) -> CGSize {
        guard image.width > 0, image.height > 0, bounds.width > 0, bounds.height > 0 else { return .zero }
        let scale = min(bounds.width / image.width, bounds.height / image.height)
        return CGSize(width: max(1, floor(image.width * scale)), height: max(1, floor(image.height * scale)))
    }

    static func normalised(_ point: CGPoint, in size: CGSize) -> CGPoint {
        guard size.width > 0, size.height > 0 else { return .zero }
        return CGPoint(x: min(max(point.x / size.width, 0), 1), y: min(max(point.y / size.height, 0), 1))
    }

    static func denormalised(_ point: CGPoint, in size: CGSize) -> CGPoint {
        CGPoint(x: point.x * size.width, y: point.y * size.height)
    }

    static func uiColor(_ hex: String) -> UIColor {
        var text = hex.hasPrefix("#") ? String(hex.dropFirst()) : hex
        if text.count == 3 { text = text.map { "\($0)\($0)" }.joined() }
        guard text.count == 6, let value = UInt32(text, radix: 16) else { return .systemRed }
        return UIColor(red: CGFloat((value >> 16) & 0xff) / 255, green: CGFloat((value >> 8) & 0xff) / 255,
                       blue: CGFloat(value & 0xff) / 255, alpha: 1)
    }

    static func color(_ hex: String) -> Color { Color(uiColor: uiColor(hex)) }

    private static func rect(_ a: CGPoint, _ b: CGPoint) -> CGRect {
        CGRect(x: min(a.x, b.x), y: min(a.y, b.y), width: abs(b.x - a.x), height: abs(b.y - a.y))
    }

    /// Tail-to-head triangle; nil when the drag is too short to orient an arrowhead.
    private static func head(_ tail: CGPoint, _ tip: CGPoint, lineWidth: CGFloat)
        -> (base: CGPoint, left: CGPoint, right: CGPoint)? {
        let dx = tip.x - tail.x, dy = tip.y - tail.y
        let length = hypot(dx, dy)
        guard length > 0.001 else { return nil }
        let ux = dx / length, uy = dy / length
        let depth = min(length, max(lineWidth * 3.2, 8))
        let half = depth * 0.42
        let base = CGPoint(x: tip.x - ux * depth, y: tip.y - uy * depth)
        return (base, CGPoint(x: base.x - uy * half, y: base.y + ux * half),
                CGPoint(x: base.x + uy * half, y: base.y - ux * half))
    }

    static func strokePath(_ shape: SFShape, in size: CGSize, lineWidth: CGFloat) -> Path {
        var path = Path()
        let points = shape.points.map { denormalised($0, in: size) }
        guard let first = points.first, let last = points.last else { return path }
        switch shape.tool {
        case .pen:
            guard points.count > 1 else { return path }
            path.addLines(points)
        case .rectangle:
            path.addRect(rect(first, last))
        case .circle:
            path.addEllipse(in: rect(first, last))
        case .arrow:
            path.move(to: first)
            path.addLine(to: head(first, last, lineWidth: lineWidth)?.base ?? last)
        }
        return path
    }

    static func headPath(_ shape: SFShape, in size: CGSize, lineWidth: CGFloat) -> Path? {
        guard shape.tool == .arrow, let first = shape.points.first, let last = shape.points.last,
              let head = head(denormalised(first, in: size), denormalised(last, in: size),
                              lineWidth: lineWidth) else { return nil }
        var path = Path()
        path.move(to: denormalised(last, in: size))
        path.addLine(to: head.left); path.addLine(to: head.right); path.closeSubpath()
        return path
    }

    static func draw(_ shapes: [SFShape], in context: inout GraphicsContext,
                     size: CGSize, lineWidth: CGFloat) {
        let style = StrokeStyle(lineWidth: lineWidth, lineCap: .round, lineJoin: .round)
        for shape in shapes {
            let tint = GraphicsContext.Shading.color(color(shape.color))
            context.stroke(strokePath(shape, in: size, lineWidth: lineWidth), with: tint, style: style)
            if let head = headPath(shape, in: size, lineWidth: lineWidth) { context.fill(head, with: tint) }
        }
    }

    /// Composites shapes onto a copy of the capture at its native pixel size. The base image is
    /// never mutated, so re-opening the editor always draws on the clean screenshot.
    static func render(_ image: UIImage, shapes: [SFShape]) -> UIImage? {
        let size = CGSize(width: max(1, floor(image.size.width * image.scale)),
                          height: max(1, floor(image.size.height * image.scale)))
        guard !shapes.isEmpty, size.width > 1 || size.height > 1 else { return nil }
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1; format.opaque = true
        let width = lineWidth(imageWidth: size.width)
        return UIGraphicsImageRenderer(size: size, format: format).image { context in
            image.draw(in: CGRect(origin: .zero, size: size))
            let cg = context.cgContext
            cg.setLineCap(.round); cg.setLineJoin(.round); cg.setLineWidth(width)
            for shape in shapes {
                let tint = uiColor(shape.color).cgColor
                cg.setStrokeColor(tint); cg.setFillColor(tint)
                let path = strokePath(shape, in: size, lineWidth: width).cgPath
                if !path.isEmpty { cg.addPath(path); cg.strokePath() }
                if let head = headPath(shape, in: size, lineWidth: width)?.cgPath, !head.isEmpty {
                    cg.addPath(head); cg.fillPath()
                }
            }
        }
    }
}

@MainActor
private struct SFMarkupEditor: View {
    let image: UIImage
    let cancel: () -> Void
    let commit: ([SFShape]) -> Void
    @State private var shapes: [SFShape]
    @State private var drafting: SFShape?
    @State private var tool: SFMarkupTool = .pen
    @State private var color = SFMarkup.colors[0]

    init(image: UIImage, shapes: [SFShape],
         cancel: @escaping () -> Void, commit: @escaping ([SFShape]) -> Void) {
        self.image = image; self.cancel = cancel; self.commit = commit
        _shapes = State(initialValue: shapes)
    }

    var body: some View {
        ZStack {
            Color.black.ignoresSafeArea()
            VStack(spacing: 0) {
                header
                GeometryReader { geometry in
                    let fitted = SFMarkup.fit(image.size, in: geometry.size)
                    canvas(fitted)
                        .frame(width: fitted.width, height: fitted.height)
                        .position(x: geometry.size.width / 2, y: geometry.size.height / 2)
                }
                .padding(.horizontal, 8)
                tools
            }
        }
        .preferredColorScheme(.dark)
    }

    private var header: some View {
        HStack {
            Button("Cancel") { cancel() }.accessibilityLabel("Cancel markup")
            Spacer()
            Text("Mark up").font(.headline)
            Spacer()
            Button("Done") { commit(shapes) }.fontWeight(.semibold)
                .accessibilityLabel("Done marking up")
        }
        .padding(.horizontal, 16).padding(.vertical, 12)
        .foregroundStyle(.white)
    }

    private func canvas(_ fitted: CGSize) -> some View {
        let width = SFMarkup.lineWidth(imageWidth: image.size.width)
            * (image.size.width > 0 ? fitted.width / image.size.width : 1)
        return ZStack {
            Image(uiImage: image).resizable().frame(width: fitted.width, height: fitted.height)
            Canvas { context, size in
                SFMarkup.draw(shapes + [drafting].compactMap { $0 },
                              in: &context, size: size, lineWidth: width)
            }
            .frame(width: fitted.width, height: fitted.height)
        }
        .contentShape(Rectangle())
        // minimumDistance 0 so a stroke starts on touch-down; a tap that never moves draws nothing.
        .gesture(DragGesture(minimumDistance: 0).onChanged { value in
            update(value, in: fitted)
        }.onEnded { value in
            let shape = drafting
            drafting = nil
            guard moved(value), let shape, shape.points.count > 1 else { return }
            shapes.append(shape)
        })
        .accessibilityLabel("Screenshot markup canvas")
        .accessibilityHint("Drag to draw with the selected tool.")
    }

    private var tools: some View {
        VStack(spacing: 14) {
            HStack(spacing: 10) {
                ForEach(SFMarkupTool.allCases) { option in
                    Button { tool = option } label: {
                        Image(systemName: option.symbol).font(.system(size: 17, weight: .semibold))
                            .frame(maxWidth: .infinity, minHeight: 38)
                            .background(tool == option ? Color.white.opacity(0.22) : Color.white.opacity(0.06),
                                        in: RoundedRectangle(cornerRadius: 9))
                    }
                    .buttonStyle(.plain).foregroundStyle(.white)
                    .accessibilityLabel(option.title)
                    .accessibilityAddTraits(tool == option ? .isSelected : [])
                }
            }
            HStack(spacing: 14) {
                ForEach(SFMarkup.colors, id: \.self) { swatch in
                    Button { color = swatch } label: {
                        Circle().fill(SFMarkup.color(swatch)).frame(width: 26, height: 26)
                            .overlay(Circle().strokeBorder(.white, lineWidth: color == swatch ? 3 : 0))
                    }
                    .buttonStyle(.plain)
                    .accessibilityLabel(swatch == SFMarkup.colors[0] ? "Red"
                                        : swatch == SFMarkup.colors[1] ? "Yellow" : "Blue")
                    .accessibilityAddTraits(color == swatch ? .isSelected : [])
                }
                Spacer()
                Button { if !shapes.isEmpty { shapes.removeLast() } } label: {
                    Label("Undo", systemImage: "arrow.uturn.backward")
                }.disabled(shapes.isEmpty).accessibilityLabel("Undo last shape")
                Button(role: .destructive) { shapes = [] } label: { Text("Clear") }
                    .disabled(shapes.isEmpty).accessibilityLabel("Clear all shapes")
            }
            .font(.subheadline).foregroundStyle(.white)
        }
        .padding(.horizontal, 16).padding(.top, 12).padding(.bottom, 8)
    }

    private func moved(_ value: DragGesture.Value) -> Bool {
        hypot(value.translation.width, value.translation.height) > 3
    }

    private func update(_ value: DragGesture.Value, in fitted: CGSize) {
        let start = SFMarkup.normalised(value.startLocation, in: fitted)
        let current = SFMarkup.normalised(value.location, in: fitted)
        guard tool == .pen else {
            drafting = SFShape(id: drafting?.id ?? UUID(), tool: tool, color: color,
                               points: [start, current])
            return
        }
        guard var shape = drafting else {
            drafting = SFShape(tool: .pen, color: color, points: [start])
            return
        }
        // Thin the polyline: sub-pixel samples add bytes without adding shape.
        let last = SFMarkup.denormalised(shape.points[shape.points.count - 1], in: fitted)
        guard hypot(value.location.x - last.x, value.location.y - last.y) > 1 else { return }
        shape.points.append(current)
        drafting = shape
    }
}

// MARK: - Best-effort diagnostics (unified logs run only in a detached task)

private struct SFLogLine: Sendable {
    let date: Date
    var line: String
}

private enum SFDiagnostics {
    nonisolated static func format(date: Date, level: String, source: String, text: String) -> String {
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.dateFormat = "HH:mm:ss.SSS"
        func singleLine(_ value: String) -> String {
            value.components(separatedBy: .newlines).joined(separator: " ")
        }
        return "\(formatter.string(from: date)) \(singleLine(level).uppercased()) [\(singleLine(source))] \(singleLine(text))"
    }

    // UTF-8 bytes including newline separators; retain a contiguous newest suffix.
    nonisolated static func bounded(_ entries: [SFLogLine]) -> [SFLogLine] {
        var result: [SFLogLine] = []
        var bytes = 0
        for entry in entries.suffix(200).reversed() {
            let remaining = 40_000 - bytes - (result.isEmpty ? 0 : 1)
            guard remaining > 0 else { break }
            var line = entry.line
            if line.utf8.count > remaining {
                // Only truncate an oversized newest line; do not leave partial older lines.
                guard result.isEmpty else { break }
                var prefix = ""
                var count = 0
                for scalar in line.unicodeScalars {
                    let next = String(scalar)
                    guard count + next.utf8.count <= remaining else { break }
                    prefix += next; count += next.utf8.count
                }
                line = prefix
            }
            bytes += line.utf8.count + (result.isEmpty ? 0 : 1)
            result.append(SFLogLine(date: entry.date, line: line))
        }
        return result.reversed()
    }

    nonisolated static func collect(breadcrumbs: [SFLogLine], until end: Date) -> [String] {
        var unified: [SFLogLine] = []
        do {
            let store = try OSLogStore(scope: .currentProcessIdentifier)
            let start = end.addingTimeInterval(-600)
            func read(includeInfo: Bool) throws -> (lines: [SFLogLine], count: Int) {
                // Scope already restricts this store to our process. Filter typed levels below
                // rather than relying on predicate aliases for the default/notice level.
                let entries = try store.getEntries(at: store.position(date: start))
                var result: [SFLogLine] = []
                var count = 0
                var indexByKey: [String: Int] = [:], basesByKey: [String: String] = [:], repeatsByKey: [String: Int] = [:]
                for case let entry as OSLogEntryLog in entries {
                    guard entry.date >= start, entry.date <= end else { continue }
                    let level: String
                    switch entry.level {
                    case .notice: level = "NOTICE"
                    case .error: level = "ERROR"
                    case .fault: level = "FAULT"
                    case .info where includeInfo: level = "INFO"
                    default: continue
                    }
                    // Apple's own subsystems (UIKit, TextInput, CoreAnimation…) chatter constantly at
                    // notice level and would drown the app's lines; keep only their errors and faults.
                    if isSystemSubsystem(entry.subsystem), entry.level != .error, entry.level != .fault { continue }
                    // Redacted / uncomposable messages carry no information at all.
                    if isNoise(entry.subsystem, entry.composedMessage) { continue }
                    // Collapse repeats (consecutive or not) into one line with a count.
                    let key = "\(level)|\(entry.subsystem):\(entry.category)|\(entry.composedMessage)"
                    if let index = indexByKey[key], index < result.count {
                        let n = (repeatsByKey[key] ?? 1) + 1
                        repeatsByKey[key] = n
                        result[index].line = (basesByKey[key] ?? result[index].line) + " (×\(n))"
                        continue
                    }
                    let base = format(date: entry.date, level: level,
                                      source: "\(entry.subsystem):\(entry.category)", text: entry.composedMessage)
                    count += 1
                    result.append(SFLogLine(date: entry.date, line: base))
                    indexByKey[key] = result.count - 1; basesByKey[key] = base; repeatsByKey[key] = 1
                    if result.count > 400 {
                        result.removeFirst(result.count - 400)
                        indexByKey = [:]; basesByKey = [:]; repeatsByKey = [:]
                    }
                }
                return (bounded(result), count)
            }
            let primary = try read(includeInfo: false)
            unified = primary.lines
            if primary.count < 50 { unified = (try? read(includeInfo: true))?.lines ?? unified }
        } catch { /* OS log access is best-effort; breadcrumbs still travel. */ }
        // The app's own breadcrumbs are the most valuable lines: reserve their room before
        // the system log fills the cap.
        let room = max(50, 200 - breadcrumbs.count)
        return bounded((breadcrumbs + unified.suffix(room)).sorted { $0.date < $1.date }).map(\.line)
    }

    /// Apple-internal subsystems are noise unless something actually went wrong.
    nonisolated static func isSystemSubsystem(_ subsystem: String) -> Bool {
        subsystem.isEmpty || subsystem.hasPrefix("com.apple.")
    }

    /// Lines the unified log could not render (privacy-redacted or uncomposable) say nothing
    /// about the app; entries with no subsystem at all are OS internals.
    nonisolated static func isNoise(_ subsystem: String, _ message: String) -> Bool {
        subsystem.isEmpty || message.hasPrefix("<compose failure") || message == "<private>"
    }

    nonisolated static func memoryMB() -> UInt64? {
        var info = task_vm_info_data_t()
        var count = mach_msg_type_number_t(MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size)
        let result = withUnsafeMutablePointer(to: &info) { pointer in
            pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
                task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
            }
        }
        guard result == KERN_SUCCESS else { return nil }
        return info.phys_footprint / (1024 * 1024)
    }

    /// Potentially blocking resource probes belong to the detached diagnostics task.
    nonisolated static func resourceMetadata() -> [String: String] {
        var values: [String: String] = [:]
        if let memory = memoryMB() { values["memoryMB"] = String(memory) }
        return values
    }
}

// MARK: - Durable reports; pure helpers are internal for @testable imports

struct SFReport: Codable, Sendable, Equatable {
    var id = UUID()
    // Canonical millisecond precision survives Date's reference-epoch conversion in JSON.
    var createdAt = Date(timeIntervalSince1970: floor(Date().timeIntervalSince1970 * 1000) / 1000)
    var attempts = 0
    var type: String
    var message: String
    var screenshot: String?
    var images: [String]?
    var logs: [String]?
    var meta: [String: String]
}

enum SFOutbox {
    nonisolated static func encode(_ report: SFReport) throws -> Data {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.sortedKeys]
        encoder.dateEncodingStrategy = .secondsSince1970
        return try encoder.encode(report)
    }
    nonisolated static func decode(_ data: Data) throws -> SFReport {
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .secondsSince1970
        return try decoder.decode(SFReport.self, from: data)
    }
    nonisolated static func ordering(_ reports: [SFReport]) -> [SFReport] {
        reports.sorted { ($0.createdAt, $0.id.uuidString) < ($1.createdAt, $1.id.uuidString) }
    }
    nonisolated static func trim(_ reports: [SFReport]) -> [SFReport] {
        Array(ordering(reports.filter { $0.attempts < 5 }).suffix(10))
    }
    nonisolated static func afterFailedAttempt(_ report: SFReport) -> SFReport? {
        var next = report
        next.attempts += 1
        return next.attempts >= 5 ? nil : next
    }
    nonisolated static func queueable(_ report: SFReport, status: Int?) -> SFReport {
        guard status == 413 else { return report }
        var next = report
        next.screenshot = nil; next.images = nil
        next.meta["screenshot"] = "dropped: too large"
        let suffix = " (screenshot dropped: too large)"
        if !next.message.hasSuffix(suffix) { next.message += suffix }
        return next
    }
    nonisolated static func fileName(_ report: SFReport) -> String {
        let stamp = String(max(0, Int64(report.createdAt.timeIntervalSince1970 * 1000)))
        return String(repeating: "0", count: max(0, 16 - stamp.count)) + stamp + "-" + report.id.uuidString + ".json"
    }
}

private enum SFStorage {
    nonisolated static func directory(_ child: String? = nil) -> URL? {
        guard let base = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask).first else { return nil }
        var url = base.appendingPathComponent("SuperFeedback", isDirectory: true)
        url.appendPathComponent(Bundle.main.bundleIdentifier ?? ProcessInfo.processInfo.processName, isDirectory: true)
        if let child { url.appendPathComponent(child, isDirectory: true) }
        do { try FileManager.default.createDirectory(at: url, withIntermediateDirectories: true); return url }
        catch { return nil }
    }
    nonisolated static func readLogs() -> [String] {
        guard let url = directory()?.appendingPathComponent("diagnostics.log"),
              let text = try? String(contentsOf: url, encoding: .utf8) else { return [] }
        return SFDiagnostics.bounded(text.components(separatedBy: "\n").filter { !$0.isEmpty }
            .map { SFLogLine(date: .distantPast, line: $0) }).map(\.line)
    }
}

private struct SFResponse: Decodable, Sendable {
    var ok: Bool?
    var url: String?
    var error: String?
}

private struct SFSendResult: Sendable {
    var ok: Bool
    var status: Int?
    var url: String?
    var error: String?
}

private enum SFNetwork {
    nonisolated static func send(_ report: SFReport, config: SuperFeedback.Config) async -> SFSendResult {
        do {
            var body: [String: Any] = ["repo": config.repo, "app": config.app, "type": report.type,
                                       "message": report.message, "meta": report.meta]
            if let screenshot = report.screenshot { body["screenshot"] = screenshot }
            if let images = report.images, !images.isEmpty { body["images"] = images }
            if let logs = report.logs, !logs.isEmpty { body["logs"] = logs }
            if !config.appKey.isEmpty { body["appKey"] = config.appKey }
            var request = URLRequest(url: config.backendURL.appendingPathComponent("report"))
            request.httpMethod = "POST"; request.timeoutInterval = 60
            request.setValue("application/json", forHTTPHeaderField: "Content-Type")
            request.httpBody = try JSONSerialization.data(withJSONObject: body)
            let (data, response) = try await URLSession.shared.data(for: request)
            let status = (response as? HTTPURLResponse)?.statusCode
            let decoded = try? JSONDecoder().decode(SFResponse.self, from: data)
            return SFSendResult(ok: status.map { (200..<300).contains($0) } == true && decoded?.ok == true,
                                status: status, url: decoded?.url, error: decoded?.error)
        } catch { return SFSendResult(ok: false, error: error.localizedDescription) }
    }
}

/// One disk owner; actor reentrancy is guarded by inFlight IDs and a single launch pass.
private actor SFDelivery {
    private var inFlight: Set<UUID> = []
    private var didStart = false

    func start(config: SuperFeedback.Config, crashMeta: [String: String], previousLogs: [String]) async {
        guard !didStart else { return }
        didStart = true
        // Freeze the launch snapshot so new submissions and failed crashes are not retried twice.
        let pending = prune()
        if config.captureCrashes, let crashes = SFStorage.directory("Crashes") {
            let files = ((try? FileManager.default.contentsOfDirectory(at: crashes, includingPropertiesForKeys: nil)) ?? [])
                .filter { $0.pathExtension == "crash" }.sorted { $0.lastPathComponent < $1.lastPathComponent }
            for file in files {
                guard let text = try? String(contentsOf: file, encoding: .utf8), !text.isEmpty else { continue }
                var report = SFReport(type: "crash", message: "App crashed in a previous session:\n" + String(text.prefix(8000)),
                                      logs: previousLogs, meta: crashMeta)
                // A stable ID makes crash -> outbox recovery safe if the process dies during the handoff.
                if let id = UUID(uuidString: file.deletingPathExtension().lastPathComponent) { report.id = id }
                if load().contains(where: { $0.id == report.id }) {
                    try? FileManager.default.removeItem(at: file)
                    continue
                }
                guard save(report) else { return } // Keep the original crash if disk persistence fails.
                _ = prune()
                try? FileManager.default.removeItem(at: file)
                let result = await attempt(report, config: config)
                if !result.ok { return }
            }
        }
        for report in pending {
            guard !inFlight.contains(report.id), exists(report) else { continue }
            let result = await attempt(report, config: config)
            if !result.ok { return }
        }
    }

    func submit(_ report: SFReport, config: SuperFeedback.Config) async -> SFSendResult {
        // Write-ahead also survives a process kill while URLSession is in flight.
        _ = save(report)
        _ = prune()
        return await attempt(report, config: config)
    }

    private func attempt(_ report: SFReport, config: SuperFeedback.Config) async -> SFSendResult {
        guard !inFlight.contains(report.id) else {
            return SFSendResult(ok: false, error: "Report is already in flight")
        }
        guard report.attempts < 5 else {
            remove(report)
            return SFSendResult(ok: false, error: "Attempt limit reached")
        }
        var attempted = report
        attempted.attempts += 1
        // Both launch recovery and submissions count the request durably before sending.
        guard save(attempted) else {
            return SFSendResult(ok: false, error: "Could not persist report attempt")
        }
        inFlight.insert(report.id)
        defer {
            inFlight.remove(report.id)
            _ = prune()
        }
        let result = await SFNetwork.send(attempted, config: config)
        if result.ok || attempted.attempts >= 5 { remove(attempted) }
        else {
            _ = save(SFOutbox.queueable(attempted, status: result.status))
        }
        return result
    }

    private func load() -> [SFReport] {
        guard let directory = SFStorage.directory("Outbox") else { return [] }
        let files = (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? []
        return SFOutbox.ordering(files.filter { $0.pathExtension == "json" }.compactMap { file in
            guard let data = try? Data(contentsOf: file), let report = try? SFOutbox.decode(data),
                  report.createdAt.timeIntervalSince1970.isFinite,
                  abs(report.createdAt.timeIntervalSince1970) < 1e12,
                  report.attempts >= 0, file.lastPathComponent == SFOutbox.fileName(report) else {
                try? FileManager.default.removeItem(at: file); return nil
            }
            return report
        })
    }
    private func save(_ report: SFReport) -> Bool {
        guard let directory = SFStorage.directory("Outbox"), let data = try? SFOutbox.encode(report) else { return false }
        do { try data.write(to: directory.appendingPathComponent(SFOutbox.fileName(report)), options: .atomic); return true }
        catch { return false }
    }
    private func exists(_ report: SFReport) -> Bool {
        guard let directory = SFStorage.directory("Outbox") else { return false }
        return FileManager.default.fileExists(atPath: directory.appendingPathComponent(SFOutbox.fileName(report)).path)
    }
    private func remove(_ report: SFReport) {
        guard let directory = SFStorage.directory("Outbox") else { return }
        try? FileManager.default.removeItem(at: directory.appendingPathComponent(SFOutbox.fileName(report)))
    }
    private func prune() -> [SFReport] {
        let all = load()
        let kept = SFOutbox.trim(all)
        let ids = Set(kept.map(\.id))
        // A concurrent submission must not erase the write-ahead count of an active request.
        for report in all where !ids.contains(report.id) && !inFlight.contains(report.id) { remove(report) }
        return kept
    }
}

// MARK: - Crash handlers

/// Immutable C storage, eagerly initialized before installing any handlers. The pointer is
/// never mutated or freed and remains valid until process exit; no shared mutable Swift state.
private final class SFCrashPath: @unchecked Sendable {
    static let shared = SFCrashPath()
    let bytes: UnsafePointer<CChar>?
    private init() {
        let file = SFStorage.directory("Crashes")?.appendingPathComponent(UUID().uuidString + ".crash")
        bytes = file.flatMap { strdup($0.path) }.map { UnsafePointer($0) }
    }
}

@MainActor
private enum SFCrashCapture {
    private static var installed = false
    private static var previousException: (@convention(c) (NSException) -> Void)?
    private static var previousSignals: [Int32: (@convention(c) (Int32) -> Void)] = [:]
    private static let signals: [Int32] = [SIGABRT, SIGSEGV, SIGBUS, SIGILL, SIGFPE, SIGTRAP]
    static func setEnabled(_ on: Bool) {
        guard on != installed else { return }
        if on {
            guard SFCrashPath.shared.bytes != nil else { return }
            previousException = NSGetUncaughtExceptionHandler()
            NSSetUncaughtExceptionHandler(sfExceptionHandler)
            for number in signals { previousSignals[number] = signal(number, sfSignalHandler) }
        } else {
            NSSetUncaughtExceptionHandler(previousException)
            for number in signals { signal(number, previousSignals[number] ?? SIG_DFL) }
            previousSignals = [:]
        }
        installed = on
    }
}

private func sfExceptionHandler(_ exception: NSException) {
    guard let path = SFCrashPath.shared.bytes else { return }
    let text = "\(exception.name.rawValue): \(exception.reason ?? "Unknown exception")\n"
        + exception.callStackSymbols.prefix(40).joined(separator: "\n") + "\n"
    // Exceptions may allocate; encode once before opening the file. Never access the log ring here.
    let encoded = Array(text.utf8)
    let fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0o600)
    if fd >= 0 {
        encoded.withUnsafeBytes { bytes in
            if let base = bytes.baseAddress { _ = write(fd, base, bytes.count) }
        }
        close(fd)
    }
    // Returning lets Foundation terminate normally; its subsequent SIGABRT appends to this file.
}

/// No Foundation, allocation, logging, locks, or mutable Swift globals in this handler.
/// The path is prepared during configure(); only POSIX open/write/close perform crash I/O.
private func sfSignalHandler(_ number: Int32) {
    if let path = SFCrashPath.shared.bytes {
        let fd = open(path, O_WRONLY | O_CREAT | O_APPEND, 0o600)
        if fd >= 0 {
            let text: StaticString
            switch number {
            case SIGABRT: text = "Fatal signal SIGABRT\n"
            case SIGSEGV: text = "Fatal signal SIGSEGV\n"
            case SIGBUS: text = "Fatal signal SIGBUS\n"
            case SIGILL: text = "Fatal signal SIGILL\n"
            case SIGFPE: text = "Fatal signal SIGFPE\n"
            case SIGTRAP: text = "Fatal signal SIGTRAP\n"
            default: text = "Fatal signal\n"
            }
            _ = write(fd, text.utf8Start, text.utf8CodeUnitCount)
            close(fd)
        }
    }
    signal(number, SIG_DFL)
    raise(number)
}
