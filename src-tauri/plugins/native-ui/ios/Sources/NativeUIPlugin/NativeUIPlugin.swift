import SwiftUI
import UIKit
import WebKit
import Tauri

class NativeUIPlugin: Plugin {
    static let diagnosticsKey = "dropbeam.shareDiagnostics"
    /// TestFlight, simulator and debug builds (not App Store installs).
    static var isTestBuild: Bool {
        #if DEBUG || targetEnvironment(simulator)
        return true
        #else
        return Bundle.main.appStoreReceiptURL?.lastPathComponent == "sandboxReceipt"
        #endif
    }
    private weak var webview: WKWebView?
    private static var feedbackStarted = false
    private var host: UIHostingController<AnyView>?

    override func load(webview: WKWebView) { self.webview = webview }

    @objc func activate(_ invoke: Invoke) {
        DispatchQueue.main.async {
            guard let root = self.manager.viewController, let webview = self.webview else {
                invoke.reject("The iOS view controller is not ready.")
                return
            }
            Bridge.shared.webview = webview
            PushRegistration.start()
            if self.host == nil {
                let host = UIHostingController(rootView: AnyView(RootView().environmentObject(Bridge.shared)))
                host.view.backgroundColor = .clear
                root.addChild(host)
                host.view.translatesAutoresizingMaskIntoConstraints = false
                root.view.addSubview(host.view)
                NSLayoutConstraint.activate([
                    host.view.leadingAnchor.constraint(equalTo: root.view.leadingAnchor),
                    host.view.trailingAnchor.constraint(equalTo: root.view.trailingAnchor),
                    host.view.topAnchor.constraint(equalTo: root.view.topAnchor),
                    host.view.bottomAnchor.constraint(equalTo: root.view.bottomAnchor)
                ])
                host.didMove(toParent: root)
                self.host = host
            }
            if let host = self.host { root.view.bringSubviewToFront(host.view) }
            // Anything shared to DropBeam while it wasn't running.
            ShareInbox.shared.ingestSoon()
            // Old picked/pasted copies nothing points at any more.
            PickedMedia.sweepSoon(after: 90)
            // Keep moving transfers alive in the background; re-probe the network on return.
            BackgroundTransfers.shared.start()
            // System-Bonjour LAN discovery, once first-run setup is out of the way (it
            // shares the Local Network prompt with iroh).
            Task { @MainActor in
                while Bridge.shared.onboarding || Bridge.shared.settings == nil { try? await Task.sleep(for: .seconds(2)) }
                LanDiscovery.shared.start()
            }
            webview.resignFirstResponder()
            webview.scrollView.isScrollEnabled = false
            webview.isHidden = true // JS bridge remains attached; no invisible touch surface.
            webview.alpha = 0
            webview.isUserInteractionEnabled = false
            webview.accessibilityElementsHidden = true
            #if targetEnvironment(simulator)
            // QA hook: `-feedbackPosition left,1` places the feedback button (no touch input in CI).
            let qaArgs = ProcessInfo.processInfo.arguments
            if let i = qaArgs.firstIndex(of: "-feedbackPosition"), i + 1 < qaArgs.count {
                let parts = qaArgs[i + 1].split(separator: ",")
                if parts.count == 2, let y = Double(parts[1]) {
                    UserDefaults.standard.set(["side": String(parts[0]), "y": y], forKey: "superfeedback.buttonPosition")
                }
            }
            #endif
            // MetricKit crash stacks join the crash reports (same opt-out switch).
            CrashDiagnostics.shared.setEnabled(UserDefaults.standard.object(forKey: Self.diagnosticsKey) as? Bool ?? true)
            if !Self.feedbackStarted {
                Self.feedbackStarted = true
                var feedback = SuperFeedback.Config(
                    backendURL: URL(string: "https://superfeedback.ashton-mcp-worker.workers.dev")!,
                    repo: "lman80/dropbeam", app: "DropBeam", trigger: .draggable,
                    position: .rightCenter, accent: .beam,
                    // No app/system log lines: DropBeam's logs carry file and friend names.
                    captureLogs: false,
                    // Crash reports follow Settings → Diagnostics → Share Diagnostics (opt-out).
                    captureCrashes: UserDefaults.standard.object(forKey: Self.diagnosticsKey) as? Bool ?? true,
                    meta: ["version": Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "",
                           "build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""],
                    // Ideas & roadmap tab (votes on public ideas).
                    community: true,
                    // TestFlight → App Store build with no In-App Purchase products: never show
                    // the backend's Stripe link here (App Store guideline 3.1.1).
                    support: .off,
                    // One anonymous voter ID across the owner's apps (Keychain Sharing entitlement).
                    keychainGroup: "superfeedback.shared"
                )
                // The floating button is a tester tool: on by default in TestFlight/dev
                // builds, off for App Store users (Settings → Send Feedback always works).
                feedback.defaultEnabled = Self.isTestBuild
                // Fallback room for the nav bar; the tab bar is measured live by the widget.
                feedback.reservedInsets = UIEdgeInsets(top: 44, left: 0, bottom: 64, right: 0)
                SuperFeedback.configure(feedback)
                SuperFeedback.setContext(["route": Bridge.shared.selectedTab])
                SuperFeedback.start()
            }
            #if targetEnvironment(simulator)
            // QA hook: `simctl launch <udid> <bundle> -simulateReceive name.jpg,…` replays a
            // "received://files" event for files already in Documents (Photos save flow).
            let args = ProcessInfo.processInfo.arguments
            if let i = args.firstIndex(of: "-simulateReceive"), i + 1 < args.count {
                let docs = SaveFolder.defaultFolder
                let paths = args[i + 1].split(separator: ",").map { docs.appendingPathComponent(String($0)).path }
                DispatchQueue.main.asyncAfter(deadline: .now() + 3) {
                    Bridge.shared.event(name: "received://files", payload: ["id": "qa", "paths": paths, "chat": false])
                }
            }
            // QA hook: `-resetOnboarding` shows first-run setup again (`-onboardingStep N` jumps).
            if args.contains("-resetOnboarding") { Bridge.shared.onboarding = true }
            // QA hook: `-openChat <friendId>` opens a conversation.
            if let i = args.firstIndex(of: "-openChat"), i + 1 < args.count {
                let id = args[i + 1]
                DispatchQueue.main.asyncAfter(deadline: .now() + 2.5) { Task { try? await Bridge.shared.openChat(friendId: id) } }
            }
            // QA hook: `-feedbackCapture <seconds>` opens the feedback panel (its screenshot
            // is also written to Documents/qa-feedback.png for inspection); `-feedbackIdeas`
            // opens it on the Ideas tab.
            if let i = args.firstIndex(of: "-feedbackCapture"), i + 1 < args.count, let delay = Double(args[i + 1]) {
                let tab: SuperFeedback.Tab = args.contains("-feedbackIdeas") ? .ideas : .feedback
                DispatchQueue.main.asyncAfter(deadline: .now() + delay) { SuperFeedback.present(tab: tab) }
            }
            // QA hook: `-forceDark` renders in dark mode regardless of the simulator setting.
            if args.contains("-forceDark") { root.view.window?.overrideUserInterfaceStyle = .dark }
            // QA hook: `-previewTransferServer` seeds Transfer Server states (no server in the simulator).
            TransferServerPreview.seedIfRequested()
            EverydayPreview.seedIfRequested()
            // QA hook: `-openTab settings` starts on a tab (screenshots without touch input).
            if let i = args.firstIndex(of: "-openTab"), i + 1 < args.count {
                let tab = args[i + 1]
                DispatchQueue.main.asyncAfter(deadline: .now() + 1) { Bridge.shared.selectedTab = tab }
            }
            #endif
            invoke.resolve()
        }
    }
    @objc func pickFiles(_ invoke: Invoke) {
        DispatchQueue.main.async {
            Task { @MainActor in
                do { let paths = try await NativeFolderPicker.shared.pickFiles(); invoke.resolve(["paths": paths]) }
                catch { invoke.reject(error.localizedDescription) }
            }
        }
    }
    @objc func pickFolder(_ invoke: Invoke) {
        let args = (try? JSONSerialization.jsonObject(with: Data(invoke.getRawArgs().utf8))) as? [String: Any]
        let upload = args?["purpose"] as? String == "upload"
        DispatchQueue.main.async {
            Task { @MainActor in
                do {
                    let path: String?
                    if upload { path = try await NativeFolderPicker.shared.pickFolderToSend() } else { path = try await NativeFolderPicker.shared.pick() }
                    invoke.resolve(["path": path as Any? ?? NSNull()])
                }
                catch { invoke.reject(error.localizedDescription) }
            }
        }
    }
    /// Live transfer activity from the plugin's Rust half (works while the WebView sleeps).
    @objc func transferActivity(_ invoke: Invoke) {
        let args = (try? JSONSerialization.jsonObject(with: Data(invoke.getRawArgs().utf8))) as? [String: Any] ?? [:]
        let activity = BackgroundTransfers.Activity(active: (args["active"] as? NSNumber)?.intValue ?? 0,
                                                    sending: (args["sending"] as? NSNumber)?.intValue ?? 0,
                                                    done: (args["bytesDone"] as? NSNumber)?.int64Value ?? 0,
                                                    total: (args["bytesTotal"] as? NSNumber)?.int64Value ?? 0)
        invoke.resolve()
        DispatchQueue.main.async { BackgroundTransfers.shared.update(activity) }
    }
    @objc func reply(_ invoke: Invoke) {
        onMain(invoke) { args in
            guard let id = args["id"] as? Int, let ok = args["ok"] as? Bool else { throw self.invalidArgs() }
            Bridge.shared.reply(id: id, ok: ok, value: args["value"] ?? NSNull())
        }
    }
    /// Store snapshots are decoded on a serial background queue (a big history or thread
    /// no longer stalls scrolling), then applied on the main thread in arrival order.
    private static let snapshotQueue = DispatchQueue(label: "dropbeam.native.snapshots", qos: .userInitiated)
    @objc func state(_ invoke: Invoke) {
        let raw = Data(invoke.getRawArgs().utf8)
        Self.snapshotQueue.async {
            do {
                let decoded = try Bridge.decodeSnapshot(raw: raw)
                DispatchQueue.main.async {
                    MainActor.assumeIsolated { Bridge.shared.apply(key: decoded.key, decoded.snapshot) }
                    invoke.resolve()
                }
            } catch { invoke.reject(error.localizedDescription) }
        }
    }
    @objc func event(_ invoke: Invoke) {
        onMain(invoke) { args in
            guard let name = args["name"] as? String else { throw self.invalidArgs() }
            let payload = args["payload"] ?? NSNull()
            Bridge.shared.event(name: name, payload: payload)
        }
    }
    private func onMain(_ invoke: Invoke, action: @escaping @MainActor ([String: Any]) throws -> Void) {
        DispatchQueue.main.async {
            do {
                let data = Data(invoke.getRawArgs().utf8)
                guard let args = try JSONSerialization.jsonObject(with: data) as? [String: Any] else { throw self.invalidArgs() }
                try action(args)
                invoke.resolve()
            } catch { invoke.reject(error.localizedDescription) }
        }
    }
    private func invalidArgs() -> NSError { NSError(domain: "DropBeam.NativeUI", code: 2, userInfo: [NSLocalizedDescriptionKey: "Invalid bridge arguments."]) }
}

@_cdecl("init_plugin_native_ui")
func initPlugin() -> Plugin { NativeUIPlugin() }
