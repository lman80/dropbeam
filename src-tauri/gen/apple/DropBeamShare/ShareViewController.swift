import UIKit
import SwiftUI

/// Entry point of the DropBeam share extension (Share → DropBeam in any app).
///
/// The engine can't run here (extension memory cap, no iroh), so this sheet only:
/// 1. copies what was shared into the App Group (starting the moment it opens),
/// 2. lets the user pick a friend from the snapshot the app keeps there,
/// 3. writes a job manifest and opens DropBeam, which sends it (ShareInbox.swift).
@objc(ShareViewController)
final class ShareViewController: UIViewController {
    private var model: ShareModel?

    override func viewDidLoad() {
        super.viewDidLoad()
        let items = (extensionContext?.inputItems as? [NSExtensionItem]) ?? []
        let model = ShareModel(items: items)
        model.onFinish = { [weak self] outcome in self?.finish(outcome) }
        self.model = model
        let host = UIHostingController(rootView: ShareRootView(model: model))
        addChild(host)
        host.view.frame = view.bounds
        host.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(host.view)
        host.didMove(toParent: self)
        model.start()
    }

    private func finish(_ outcome: ShareModel.Outcome) {
        switch outcome {
        case .cancelled:
            extensionContext?.cancelRequest(withError: NSError(domain: NSCocoaErrorDomain, code: NSUserCancelledError))
        case .done:
            extensionContext?.completeRequest(returningItems: nil)
        case .open(let url):
            // Bring DropBeam forward so it sends right away. If iOS refuses, the job still
            // waits in the App Group and goes out the next time DropBeam opens.
            openHostApp(url) { [weak self] opened in
                guard let self else { return }
                if opened { self.extensionContext?.completeRequest(returningItems: nil) }
                else { self.model?.openFailed() }
            }
        }
    }

    /// Extensions have no `UIApplication.shared`; the share sheet's responder chain still
    /// reaches the app object, whose `openURL:options:completionHandler:` works for our own
    /// registered scheme (the only thing we ever open).
    private func openHostApp(_ url: URL, completion: @escaping (Bool) -> Void) {
        let selector = NSSelectorFromString("openURL:options:completionHandler:")
        var responder: UIResponder? = self
        while let current = responder {
            if let app = current as? UIApplication, app.responds(to: selector), let imp = app.method(for: selector) {
                typealias Open = @convention(c) (AnyObject, Selector, NSURL, NSDictionary, (@convention(block) (Bool) -> Void)?) -> Void
                let open = unsafeBitCast(imp, to: Open.self)
                let done: @convention(block) (Bool) -> Void = { ok in DispatchQueue.main.async { completion(ok) } }
                open(app, selector, url as NSURL, NSDictionary(), done)
                return
            }
            responder = current.next
        }
        completion(false)
    }
}
