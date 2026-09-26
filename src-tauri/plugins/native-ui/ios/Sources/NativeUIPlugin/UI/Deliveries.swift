import SwiftUI

/// Where a send to a friend is on ONE of their devices (their Mac, their iPhone…).
/// Mirrors `Delivery` in src/lib/api.ts; the words mirror src/lib/deliveries.ts.
struct Delivery: Decodable, Identifiable, Equatable {
    let eid: String
    var label: String
    var kind: String?
    var os: String?
    var state: String
    var via: String?
    var note: String?
    var id: String { eid }

    init(from decoder: Decoder) throws {
        let c = try decoder.container(keyedBy: Key.self)
        eid = try c.decode(String.self, forKey: .eid)
        label = (try? c.decode(String.self, forKey: .label)) ?? "Device"
        kind = try? c.decode(String.self, forKey: .kind)
        os = try? c.decode(String.self, forKey: .os)
        state = (try? c.decode(String.self, forKey: .state)) ?? "sending"
        via = (try? c.decode(String.self, forKey: .via)).flatMap { $0.isEmpty ? nil : $0 }
        note = (try? c.decode(String.self, forKey: .note)).flatMap { $0.isEmpty ? nil : $0 }
    }
    init(eid: String, label: String, kind: String? = nil, os: String? = nil, state: String, via: String? = nil, note: String? = nil) {
        self.eid = eid; self.label = label; self.kind = kind; self.os = os; self.state = state; self.via = via; self.note = note
    }
    private enum Key: String, CodingKey { case eid, label, kind, os, state, via, note }

    var symbol: String {
        switch os {
        case "ios": return kind == "tablet" ? "ipad" : "iphone"
        case "macos": return kind == "desktop" ? "desktopcomputer" : "laptopcomputer"
        default: return kind == "phone" ? "iphone" : kind == "laptop" ? "laptopcomputer" : "desktopcomputer"
        }
    }
}

enum DeliveryCopy {
    static func multi(_ ds: [Delivery]?) -> [Delivery]? { (ds?.count ?? 0) > 1 ? ds : nil }

    private static func firstName(_ name: String) -> String {
        name.split(separator: " ").first.map(String.init) ?? name
    }
    private static func joinAnd(_ xs: [String]) -> String {
        switch xs.count {
        case 0: return ""
        case 1: return xs[0]
        case 2: return "\(xs[0]) and \(xs[1])"
        default: return xs.dropLast().joined(separator: ", ") + " and " + xs[xs.count - 1]
        }
    }
    /// A reason we only know as a code, in words; engine errors read as nothing.
    private static func noteText(_ note: String?) -> String? {
        guard let note, !note.isEmpty else { return nil }
        switch note {
        case "files_gone": return "the files were moved"
        case "expired": return "waited too long"
        case "refused": return "it couldn’t open it"
        default:
            let human = note.count <= 61 && !note.contains("_") && !note.contains(":") && note.first?.isUppercase == true
            return human ? note : nil
        }
    }
    private static func phrase(_ d: Delivery, plural: Bool) -> String {
        let its = plural ? "they’re" : "it’s"
        switch d.state {
        case "delivered": return "delivered"
        case "held": return "waiting (\(d.via ?? "your Transfer Server") is holding it)"
        case "waiting":
            if let why = noteText(d.note) { return "waiting (\(why.prefix(1).lowercased() + why.dropFirst()))" }
            return "waiting — sends when \(its) online"
        case "uploading": return "uploading to \(d.via ?? "your Transfer Server")"
        case "offline": return "not reachable yet"
        case "declined": return "declined"
        case "canceled": return "canceled"
        case "paused": return "paused"
        case "failed": return noteText(d.note).map { "couldn’t deliver — \($0)" } ?? "couldn’t deliver"
        default: return "sending"
        }
    }
    /// One device's status for its own row.
    static func status(_ d: Delivery) -> String {
        switch d.state {
        case "delivered": return "Delivered"
        case "held": return "Waiting · \(d.via ?? "your Transfer Server") is holding it"
        case "waiting": return noteText(d.note).map { "Waiting · \($0)" } ?? "Waiting · sends when it’s online"
        case "uploading": return "Uploading to \(d.via ?? "your Transfer Server")"
        case "offline": return "Not reachable yet"
        case "declined": return "Declined"
        case "canceled": return "Canceled"
        case "paused": return "Paused"
        case "failed": return noteText(d.note).map { "Couldn’t deliver · \($0)" } ?? "Couldn’t deliver"
        default: return "Sending"
        }
    }
    static func problem(_ ds: [Delivery]) -> Bool { ds.contains { $0.state == "failed" || $0.state == "declined" } }

    /// "Delivered to Alex’s Mac · iPhone: waiting (Linux Box is holding it)"
    static func summary(friend: String, _ ds: [Delivery]) -> String {
        let who = firstName(friend)
        let done = ds.filter { $0.state == "delivered" }
        if !ds.isEmpty && done.count == ds.count {
            return ds.count > 3 ? "Delivered to all \(ds.count) of \(who)’s devices" : "Delivered to \(who)’s \(joinAnd(done.map(\.label)))"
        }
        var parts: [String] = []
        if !done.isEmpty { parts.append("Delivered to \(who)’s \(joinAnd(done.map(\.label)))") }
        var order: [String] = []
        var groups: [String: [Delivery]] = [:]
        for d in ds where d.state != "delivered" {
            let key = phrase(d, plural: false)
            if groups[key] == nil { order.append(key) }
            groups[key, default: []].append(d)
        }
        for key in order {
            let group = groups[key] ?? []
            let clause = "\(joinAnd(group.map(\.label))): \(phrase(group[0], plural: group.count > 1))"
            parts.append(parts.isEmpty ? "\(who)’s \(clause)" : clause)
        }
        return parts.joined(separator: " · ")
    }
}

/// A person's devices for "Send to one device": the record that owns their
/// conversation plus the extra devices grouped under it.
func personDevices(_ friend: Friend, in all: [Friend]) -> [(eid: String, label: String, symbol: String)] {
    let members = all.filter { ($0.id == friend.id || $0.groupedUnder == friend.id) && ($0.endpointId ?? "").isEmpty == false }
    let byEid = members.sorted { ($0.endpointId ?? "") < ($1.endpointId ?? "") }
    return members.map { f in
        let noun = deviceNoun(f.deviceKind, os: f.deviceOs)
        let same = byEid.filter { deviceNoun($0.deviceKind, os: $0.deviceOs) == noun }
        let label = same.count > 1 ? "\(noun) \((same.firstIndex { $0.id == f.id } ?? 0) + 1)" : noun
        let d = Delivery(eid: f.endpointId ?? "", label: label, kind: f.deviceKind, os: f.deviceOs, state: "sending")
        return (d.eid, label, d.symbol)
    }
}

/// One row per device under a transfer: "􀟜 Mac  Delivered ✓".
struct DeliveryRows: View {
    let deliveries: [Delivery]
    var body: some View {
        VStack(alignment: .leading, spacing: 6) {
            ForEach(deliveries) { d in
                HStack(spacing: 8) {
                    Image(systemName: d.symbol).font(.footnote).foregroundStyle(.secondary).frame(width: 20)
                    Text(d.label).font(.footnote.weight(.semibold))
                    Text(DeliveryCopy.status(d)).font(.footnote)
                        .foregroundStyle(["failed", "declined"].contains(d.state) ? Color.red : .secondary).lineLimit(1)
                    Spacer(minLength: 0)
                    if d.state == "delivered" { Image(systemName: "checkmark").font(.caption.weight(.bold)).foregroundStyle(.green) }
                }
                .accessibilityElement(children: .combine)
            }
        }
    }
}
