// Used by native-media-fixtures.swift with the `bridge` launch argument.
// Exercises the real Bridge through a hidden WKWebView, without Tauri/network.
import WebKit

@MainActor private final class PickerReplyFixture: NSObject, WKScriptMessageHandler {
    var value: Any = [String]()
    var ok = true
    var pending = false
    var lastID = 0
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage) {
        guard let args = message.body as? [String: Any], let id = args["id"] as? Int else { return }
        lastID = id
        if !pending { Bridge.shared.reply(id: id, ok: ok, value: value) }
    }
}

@MainActor func runNativePickerChecks(paths: [String]) async {
    let handler = PickerReplyFixture()
    let configuration = WKWebViewConfiguration()
    configuration.userContentController.add(handler, name: "fixture")
    let web = WKWebView(frame: .zero, configuration: configuration)
    web.isHidden = true; web.isUserInteractionEnabled = false
    let window = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows).first { $0.isKeyWindow }
    window?.addSubview(web)
    web.loadHTMLString("<script>window.__dbBridge={call:(id,name,args)=>window.webkit.messageHandlers.fixture.postMessage({id,name,args})}</script>", baseURL: nil)
    for _ in 0..<60 {
        if (try? await web.evaluateJavaScript("typeof window.__dbBridge")) as? String == "object" { break }
        try? await Task.sleep(for: .milliseconds(100))
    }
    let bridge = Bridge.shared
    bridge.webview = web
    defer { bridge.webview = nil; configuration.userContentController.removeScriptMessageHandler(forName: "fixture"); web.removeFromSuperview() }
    do {
        handler.value = paths
        let selected = try await bridge.pickFiles(source: "photos")
        precondition(selected == paths && bridge.preparingMedia == nil)
        handler.value = [String]()
        let canceled = try await bridge.pickFiles(source: "files")
        precondition(canceled.isEmpty && bridge.preparingMedia == nil)
        handler.ok = false; handler.value = "Fixture provider failure"
        do { _ = try await bridge.pickFiles(source: "photos"); preconditionFailure("Expected error") }
        catch { precondition(bridge.preparingMedia == nil) }
        handler.pending = true; handler.ok = true; handler.value = paths
        let task = Task { try await bridge.pickFiles(source: "photos") }
        try? await Task.sleep(for: .milliseconds(200))
        let canceledID = handler.lastID
        bridge.cancelMediaPreparation()
        do { _ = try await task.value; preconditionFailure("Expected cancellation") }
        catch { precondition(bridge.preparingMedia == nil) }
        bridge.reply(id: canceledID, ok: true, value: paths)
        precondition(bridge.preparingMedia == nil)
        fixtureLog("PASS: picker success (14), empty cancel, error, explicit cancel, late reply")
        let start = Date()
        do { _ = try await bridge.pickFiles(source: "photos"); preconditionFailure("Expected timeout") }
        catch { precondition(Date().timeIntervalSince(start) >= 89 && Date().timeIntervalSince(start) < 96 && bridge.preparingMedia == nil) }
        bridge.reply(id: handler.lastID, ok: true, value: paths)
        handler.pending = false
        let retry = try await bridge.pickFiles(source: "photos")
        precondition(retry == paths && bridge.preparingMedia == nil)
        fixtureLog("PASS: 90s picker timeout, ignored late reply and successful retry")
    } catch { preconditionFailure("Picker fixture failed: \(error)") }
}
