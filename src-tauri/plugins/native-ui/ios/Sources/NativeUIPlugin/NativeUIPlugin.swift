import SwiftUI
import UIKit
import WebKit
import Tauri

class NativeUIPlugin: Plugin {
    private weak var webview: WKWebView?
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
            self.setWebOverlay(false)
            invoke.resolve()
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
            if name == "webOverlay", let object = payload as? [String: Any] {
                self.setWebOverlay(object["visible"] as? Bool ?? false)
            }
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
    @MainActor private func setWebOverlay(_ visible: Bool) {
        // Phase 1: existing React recipient chooser temporarily owns the screen.
        // The WKWebView is never removed, so subscriptions and Rust IPC stay alive.
        host?.view.isHidden = visible
        webview?.alpha = visible ? 1 : 0
        webview?.isUserInteractionEnabled = visible
        webview?.accessibilityElementsHidden = !visible
    }
    private func invalidArgs() -> NSError { NSError(domain: "DropBeam.NativeUI", code: 2, userInfo: [NSLocalizedDescriptionKey: "Invalid bridge arguments."]) }
}

@_cdecl("init_plugin_native_ui")
func initPlugin() -> Plugin { NativeUIPlugin() }
