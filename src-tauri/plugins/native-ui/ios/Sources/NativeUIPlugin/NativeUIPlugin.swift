import SwiftUI
import UIKit
import WebKit
import Tauri

class NativeUIPlugin: Plugin {
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
            webview.resignFirstResponder()
            webview.scrollView.isScrollEnabled = false
            webview.alpha = 0
            webview.isUserInteractionEnabled = false
            webview.accessibilityElementsHidden = true
            if !Self.feedbackStarted {
                Self.feedbackStarted = true
                SuperFeedback.configure(.init(
                    backendURL: URL(string: "https://superfeedback.ashton-mcp-worker.workers.dev")!,
                    repo: "lman80/dropbeam", app: "DropBeam", trigger: .floating,
                    position: .rightCenter, captureLogs: false, captureCrashes: true,
                    meta: ["version": Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "",
                           "build": Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? ""]
                ))
                SuperFeedback.setContext(["screen": Bridge.shared.selectedTab])
                SuperFeedback.start()
            }
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
        DispatchQueue.main.async {
            Task { @MainActor in
                do { let path = try await NativeFolderPicker.shared.pick(); invoke.resolve(["path": path as Any? ?? NSNull()]) }
                catch { invoke.reject(error.localizedDescription) }
            }
        }
    }
    @objc func reply(_ invoke: Invoke) {
        onMain(invoke) { args in
            guard let id = args["id"] as? Int, let ok = args["ok"] as? Bool else { throw self.invalidArgs() }
            Bridge.shared.reply(id: id, ok: ok, value: args["value"] ?? NSNull())
        }
    }
    @objc func state(_ invoke: Invoke) {
        onMain(invoke) { args in
            guard let key = args["key"] as? String else { throw self.invalidArgs() }
            try Bridge.shared.update(key: key, value: args["value"] ?? NSNull())
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
