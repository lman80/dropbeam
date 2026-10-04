import Foundation
import Network
import UIKit

/// Finding DropBeam devices on the same Wi-Fi through the SYSTEM's Bonjour (T9).
///
/// iroh's own LAN discovery (swarm-discovery) sends raw multicast, which iOS only
/// allows with Apple's `com.apple.developer.networking.multicast` entitlement (granted
/// on request). Bonjour through mDNSResponder needs no entitlement — just
/// NSLocalNetworkUsageDescription + `_dropbeam._udp` in NSBonjourServices — so:
///
/// - this iPhone advertises `_dropbeam._udp` with a TXT record holding its endpoint id
///   and its LAN socket addresses (from the engine, `lan_self_info`);
/// - it browses the same service and hands every device it finds to the engine
///   (`lan_peer_found` → an in-memory address lookup), so the next dial to that device
///   goes straight over the LAN even before (or without) the relay.
///
/// Nothing here trusts the advertisement: connections are still authenticated by the
/// device's key; the engine only accepts private/link-local addresses.
/// Desktop builds keep iroh's swarm-discovery; advertising `_dropbeam._udp` from desktop
/// too is a follow-up (then iPhone ↔ Mac is found both ways).
@MainActor final class LanDiscovery {
    static let shared = LanDiscovery()
    nonisolated static let serviceType = "_dropbeam._udp"
    private var listener: NWListener?
    private var browser: NWBrowser?
    private var advertised: [String: String] = [:]
    private var myId: String?
    /// endpoint id → when it was last handed to the engine (re-sent at most every 30 s).
    private var reported: [String: (Date, [String])] = [:]
    private var running = false
    private var refreshTask: Task<Void, Never>?

    func start() {
        guard !running else { refresh(); return }
        running = true
        if !observing { observing = true; observe() }
        startBrowser()
        refresh()
    }
    private var observing = false
    private func observe() {
        NotificationCenter.default.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { _ in
            Task { @MainActor in LanDiscovery.shared.stop() }
        }
        NotificationCenter.default.addObserver(forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main) { _ in
            Task { @MainActor in LanDiscovery.shared.start() }
        }
    }

    func stop() {
        running = false
        browser?.cancel(); browser = nil
        listener?.cancel(); listener = nil
        advertised = [:]
    }

    /// Re-read our addresses (they change with the network) and re-advertise if needed.
    func refresh() {
        guard running else { return }
        refreshTask?.cancel()
        refreshTask = Task { @MainActor in
            // The engine binds its sockets shortly after launch; retry until it answers.
            for attempt in 0..<10 {
                if Task.isCancelled { return }
                if let me: LanSelf = try? await Bridge.shared.call("lanSelfInfo") {
                    advertise(me)
                    return
                }
                try? await Task.sleep(for: .seconds(attempt < 3 ? 2 : 10))
            }
        }
    }

    private struct LanSelf: Decodable { let endpointId: String; let addrs: [String] }

    nonisolated static func txt(for id: String, addrs: [String]) -> [String: String] {
        // One address per key: a TXT string is at most 255 bytes.
        var entries = ["v": "1", "id": id]
        for (index, addr) in addrs.prefix(8).enumerated() { entries["a\(index)"] = addr }
        return entries
    }
    nonisolated static func parse(_ txt: [String: String]) -> (id: String, addrs: [String])? {
        guard txt["v"] == "1", let id = txt["id"], !id.isEmpty, id.count <= 128 else { return nil }
        let addrs = (0..<8).compactMap { txt["a\($0)"] }.filter { !$0.isEmpty && $0.count <= 64 }
        return addrs.isEmpty ? nil : (id, addrs)
    }

    private func advertise(_ me: LanSelf) {
        myId = me.endpointId
        let entries = Self.txt(for: me.endpointId, addrs: me.addrs)
        guard !me.addrs.isEmpty else { listener?.cancel(); listener = nil; advertised = [:]; return }
        if entries == advertised, listener != nil { return }
        advertised = entries
        let service = NWListener.Service(name: "DropBeam-" + me.endpointId.prefix(16), type: Self.serviceType,
                                         domain: nil, txtRecord: NWTXTRecord(entries))
        if let listener {
            listener.service = service
            return
        }
        do {
            // A placeholder UDP port: the service is only a signpost; iroh's own socket
            // (in the TXT record) carries the traffic. Nothing is ever accepted here.
            let listener = try NWListener(using: .udp, on: .any)
            listener.service = service
            listener.newConnectionHandler = { $0.cancel() }
            listener.stateUpdateHandler = { state in
                if case .failed(let error) = state {
                    NSLog("DropBeam Bonjour: advertising failed: %@", "\(error)")
                    Task { @MainActor in LanDiscovery.shared.listener = nil; LanDiscovery.shared.advertised = [:] }
                }
            }
            listener.start(queue: .main)
            self.listener = listener
        } catch {
            NSLog("DropBeam Bonjour: could not advertise: %@", error.localizedDescription)
        }
    }

    private func startBrowser() {
        let browser = NWBrowser(for: .bonjourWithTXTRecord(type: Self.serviceType, domain: nil), using: .udp)
        browser.browseResultsChangedHandler = { results, _ in
            let found: [(String, [String])] = results.compactMap { result in
                guard case .bonjour(let record) = result.metadata else { return nil }
                return Self.parse(record.dictionary)
            }
            Task { @MainActor in LanDiscovery.shared.found(found) }
        }
        browser.stateUpdateHandler = { state in
            if case .failed(let error) = state { NSLog("DropBeam Bonjour: browsing failed: %@", "\(error)") }
        }
        browser.start(queue: .main)
        self.browser = browser
    }

    private func found(_ peers: [(String, [String])]) {
        let now = Date()
        for (id, addrs) in peers where id != myId {
            if let last = reported[id], last.1 == addrs, now.timeIntervalSince(last.0) < 30 { continue }
            reported[id] = (now, addrs)
            Task { _ = try? await Bridge.shared.call("lanPeerFound", ["endpointId": id, "addrs": addrs]) as Bool }
        }
    }
}
