import SwiftUI

// MARK: - Transfer Server (iOS only USES servers; hosting is desktop-only)
//
// A Transfer Server is someone's always-on computer that holds sealed messages and
// files for friends who are offline and delivers them when they're back.

/// The ~9 s looping explainer: You → a sleeping friend (bounces), waits locked on the
/// server, lands when they wake. Neutral fills; the accent only on the plane and check.
/// Reduce Motion shows one still frame plus the three steps, numbered.
struct ServerExplainer: View {
    var serverName: String? = nil
    var friendName: String? = nil
    var compact = false
    /// The surface it sits on: discs are opaque over the connecting line.
    var base = Color(uiColor: .secondarySystemGroupedBackground)
    @Environment(\.accessibilityReduceMotion) private var reduceMotion
    #if targetEnvironment(simulator)
    private var stillForQA: Bool { CommandLine.arguments.contains("-reduceMotionPreview") }
    #else
    private let stillForQA = false
    #endif
    static let steps = [
        "Your friend is offline",
        "It waits on the Transfer Server, locked so only they can open it",
        "It arrives when they’re back, then the server deletes its copy",
    ]
    static let duration = 9.0
    var body: some View {
        if reduceMotion || stillForQA {
            VStack(alignment: .leading, spacing: 14) {
                stage(Frame.still)
                VStack(alignment: .leading, spacing: 8) {
                    ForEach(Array(Self.steps.enumerated()), id: \.offset) { i, step in
                        HStack(alignment: .firstTextBaseline, spacing: 10) {
                            Text("\(i + 1)").font(.subheadline.weight(.semibold)).monospacedDigit().foregroundStyle(.secondary)
                            Text(step).font(.subheadline).fixedSize(horizontal: false, vertical: true)
                        }
                    }
                }
            }
            .accessibilityElement(children: .combine)
        } else {
            TimelineView(.animation(minimumInterval: 1 / 60)) { context in
                let t = context.date.timeIntervalSinceReferenceDate.truncatingRemainder(dividingBy: Self.duration) / Self.duration
                let frame = Frame(t)
                VStack(spacing: compact ? 10 : 14) {
                    stage(frame)
                    ZStack {
                        ForEach(0..<3) { i in
                            Text(Self.steps[i]).font(compact ? .footnote : .subheadline).foregroundStyle(.secondary)
                                .multilineTextAlignment(.center).opacity(frame.caption(i))
                        }
                        // Reserve the tallest caption so the layout never jumps.
                        Text(Self.steps[1]).font(compact ? .footnote : .subheadline).multilineTextAlignment(.center).hidden()
                    }
                    .frame(maxWidth: .infinity)
                }
            }
            .accessibilityElement(children: .ignore)
            .accessibilityLabel(Self.steps.joined(separator: ". "))
        }
    }

    private var disc: CGFloat { compact ? 44 : 54 }
    private func stage(_ f: Frame) -> some View {
        HStack(alignment: .top, spacing: 0) {
            station("iphone", "You", opacity: f.you) {
                badge("checkmark", fill: .beam, glyph: .white).opacity(f.check).scaleEffect(0.6 + 0.4 * f.checkScale)
            }
            station("server.rack", serverName ?? "Transfer Server", opacity: 1) {
                badge("lock.fill", fill: .primary, glyph: Color(uiColor: .systemBackground)).opacity(f.lock).scaleEffect(0.6 + 0.4 * f.lockScale)
            }
            station("laptopcomputer", friendName ?? "Friend", opacity: f.them) {
                Image(systemName: "zzz").font(.system(size: 11, weight: .semibold)).foregroundStyle(.secondary)
                    .offset(x: 0, y: 2 - 4 * f.zzzRise).opacity(f.zzz)
            }
        }
        .background(alignment: .topLeading) {
            GeometryReader { geo in
                Capsule().fill(Color(uiColor: .separator).opacity(0.6))
                    .frame(width: geo.size.width * 2 / 3, height: 1.5)
                    .position(x: geo.size.width / 2, y: disc / 2)
            }
        }
        .overlay(alignment: .topLeading) {
            GeometryReader { geo in
                Image(systemName: "paperplane.fill")
                    .font(.system(size: compact ? 13 : 15, weight: .semibold))
                    .foregroundStyle(Color.beam)
                    .rotationEffect(.degrees(f.planeBack ? 225 : 45))
                    .position(x: geo.size.width * f.planeX, y: disc / 2 + f.planeY)
                    .opacity(f.plane)
            }
        }
        .accessibilityHidden(true)
    }
    private func station<Badge: View>(_ symbol: String, _ label: String, opacity: Double, @ViewBuilder badge: () -> Badge) -> some View {
        VStack(spacing: 6) {
            Image(systemName: symbol)
                .font(.system(size: disc * 0.4, weight: .regular))
                .foregroundStyle(.primary)
                .frame(width: disc, height: disc)
                .background(Color(uiColor: .tertiarySystemFill), in: Circle())
                .background(base, in: Circle())
                .overlay(alignment: .topTrailing) { badge().offset(x: 4, y: -4) }
            Text(label).font(.caption).foregroundStyle(.secondary).lineLimit(1).truncationMode(.tail)
        }
        .opacity(opacity)
        .frame(maxWidth: .infinity)
    }
    private func badge(_ symbol: String, fill: Color, glyph: Color) -> some View {
        Image(systemName: symbol).font(.system(size: 9, weight: .bold)).foregroundStyle(glyph)
            .frame(width: 20, height: 20).background(fill, in: Circle())
            .overlay(Circle().stroke(Color(uiColor: .systemBackground), lineWidth: 2))
    }

    /// Every animated value at time t ∈ [0, 1) of the loop (same beats as desktop CSS).
    struct Frame {
        // The still frame: on its way from you to the server, which already holds it locked.
        var planeX = 0.335, planeY = 0.0, plane = 1.0, planeBack = false
        var you = 1.0, them = 0.45, zzz = 1.0, zzzRise = 0.0
        var lock = 1.0, lockScale = 1.0, check = 0.0, checkScale = 0.0
        var t = 0.4
        static let still = Frame()
        init() {}
        init(_ t: Double) {
            self.t = t
            planeX = Self.track(t, [(0, 0.1667), (0.04, 0.1667), (0.18, 0.72), (0.24, 0.62), (0.33, 0.62), (0.42, 0.5), (0.71, 0.5), (0.82, 0.8333), (1, 0.8333)])
            planeY = Self.arc(t, 0.04, 0.18, -12) + Self.arc(t, 0.33, 0.42, -6) + Self.arc(t, 0.71, 0.82, -12)
            planeBack = t > 0.18 && t < 0.42
            plane = Self.track(t, [(0, 0), (0.04, 1), (0.40, 1), (0.44, 0), (0.69, 0), (0.75, 1), (0.82, 1), (0.86, 0), (1, 0)])
            them = Self.track(t, [(0, 0.45), (0.66, 0.45), (0.70, 1), (0.97, 1), (1, 0.45)])
            you = Self.track(t, [(0, 1), (0.48, 1), (0.54, 0.45), (0.80, 0.45), (0.84, 1), (1, 1)])
            zzz = Self.track(t, [(0, 0), (0.06, 1), (0.60, 1), (0.66, 0), (1, 0)])
            zzzRise = Self.track(t, [(0, 0), (0.60, 1), (0.66, 1.5), (1, 1.5)])
            lock = Self.track(t, [(0, 0), (0.42, 0), (0.45, 1), (0.84, 1), (0.88, 0), (1, 0)])
            lockScale = Self.track(t, [(0, 0), (0.42, 0), (0.46, 1.25), (0.49, 1), (1, 1)])
            check = Self.track(t, [(0, 0), (0.82, 0), (0.85, 1), (0.96, 1), (1, 0)])
            checkScale = Self.track(t, [(0, 0), (0.82, 0), (0.86, 1.25), (0.89, 1), (1, 1)])
        }
        /// Caption i's opacity (three thirds, short crossfades).
        func caption(_ i: Int) -> Double {
            let a = Double(i) / 3, b = Double(i + 1) / 3, fade = 0.03
            if t < a || t > b { return 0 }
            return min(1, (t - a) / fade, (b - t) / fade)
        }
        private static func ease(_ x: Double) -> Double { x * x * (3 - 2 * x) }
        static func track(_ t: Double, _ keys: [(Double, Double)]) -> Double {
            guard let first = keys.first else { return 0 }
            if t <= first.0 { return first.1 }
            for (a, b) in zip(keys, keys.dropFirst()) where t <= b.0 {
                let span = b.0 - a.0
                return span <= 0 ? b.1 : a.1 + (b.1 - a.1) * ease((t - a.0) / span)
            }
            return keys.last!.1
        }
        private static func arc(_ t: Double, _ a: Double, _ b: Double, _ height: Double) -> Double {
            guard t > a, t < b else { return 0 }
            return height * sin(.pi * ease((t - a) / (b - a)))
        }
    }
}

/// "How it works": the explainer with two short lines of plain facts.
struct ServerExplainerSheet: View {
    @Environment(\.dismiss) private var dismiss
    var serverName: String? = nil
    var body: some View {
        NavigationStack {
            List {
                Section {
                    ServerExplainer(serverName: serverName).padding(.vertical, 12)
                }
                Section {
                    fact("moon.zzz.fill", "When someone you send to is offline, a Transfer Server holds your messages and files until they’re back.")
                    fact("lock.fill", "Everything stays locked. The server can’t open it, and deletes its copy once it’s delivered.")
                    fact("desktopcomputer", "A friend’s always-on Mac or PC can be one. iPhone can use servers but can’t be one.")
                }
            }
            .beamList()
            .navigationTitle("Transfer Servers").navigationBarTitleDisplayMode(.inline)
            .toolbar { ToolbarItem(placement: .confirmationAction) { Button("Done") { dismiss() } } }
        }
        .tint(.beam)
    }
    private func fact(_ symbol: String, _ text: String) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 12) {
            Image(systemName: symbol).foregroundStyle(.secondary).frame(width: 22).accessibilityHidden(true)
            Text(text).font(.subheadline).fixedSize(horizontal: false, vertical: true)
        }.padding(.vertical, 2)
    }
}

/// The one-time card in a friend's thread when they share their Transfer Server.
struct ServerOfferCard: View {
    @EnvironmentObject private var bridge: Bridge
    let server: UsableServer
    let friendName: String
    @State private var hold = true
    @State private var busy = false
    var body: some View {
        VStack(alignment: .leading, spacing: 14) {
            ServerExplainer(serverName: server.name, friendName: nil, compact: true, base: Color(uiColor: .secondarySystemBackground))
                .padding(.top, 6)
            VStack(alignment: .leading, spacing: 4) {
                Text("\(friendName) shared a Transfer Server with you").font(.headline)
                Text("When someone you send to is offline, \(server.name) holds it until they’re back. Everything stays locked — only they can open it.")
                    .font(.subheadline).foregroundStyle(.secondary).fixedSize(horizontal: false, vertical: true)
            }
            Toggle("Hold my messages there too", isOn: $hold).tint(.green).font(.subheadline)
            HStack(spacing: 10) {
                Button { answer(false) } label: { Text("Not Now").frame(maxWidth: .infinity) }.beamButton()
                Button { answer(true) } label: { Text("Use It").frame(maxWidth: .infinity) }.beamButton(prominent: true)
            }
            .controlSize(.large).disabled(busy)
        }
        .padding(16)
        .background(Color(uiColor: .secondarySystemBackground), in: RoundedRectangle(cornerRadius: 22, style: .continuous))
        .accessibilityElement(children: .contain)
        .accessibilityIdentifier("chat.serverOffer")
    }
    private func answer(_ use: Bool) {
        busy = true
        bridge.perform {
            defer { busy = false }
            if use {
                try await bridge.setServerPrefs(eid: server.eid, useIt: true, holdForMe: hold, offer: "seen")
                Haptics.success(); bridge.showToast("Using \(server.name)")
            } else {
                try await bridge.setServerPrefs(eid: server.eid, offer: "dismissed")
            }
        }
    }
}

/// Settings → Transfer Servers: the servers this device may use.
struct TransferServersSection: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var explaining = false
    @State private var removing: UsableServer?
    var body: some View {
        Section {
            if bridge.servers.isEmpty {
                Text("When a friend shares their Transfer Server with you, it shows up here.")
                    .font(.subheadline).foregroundStyle(.secondary).padding(.vertical, 2)
            }
            ForEach(bridge.servers) { server in
                row(server)
                if !server.own && !server.revoked && server.useIt {
                    Toggle(isOn: Binding(get: { server.holdForMe }, set: { on in
                        bridge.perform { try await bridge.setServerPrefs(eid: server.eid, holdForMe: on) }
                    })) {
                        Text("Hold My Messages Here").padding(.leading, 44)
                            .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] + 44 }
                    }
                    .tint(.green).disabled(server.paused)
                    .accessibilityLabel("Hold my messages on \(server.name)")
                }
            }
            ActionRow(title: "How It Works", symbol: "questionmark", color: .gray) { explaining = true }
                .id("transferServersEnd")
                .sheet(isPresented: $explaining) { ServerExplainerSheet().presentationDetents([.large]) }
                #if targetEnvironment(simulator)
                // QA: `-openServerExplainer` shows "How It Works".
                .task { if CommandLine.arguments.contains("-openServerExplainer") { try? await Task.sleep(for: .seconds(1.5)); explaining = true } }
                #endif
                .confirmationDialog(removing.map { "Remove \($0.name)?" } ?? "", isPresented: Binding(get: { removing != nil }, set: { if !$0 { removing = nil } }), titleVisibility: .visible, presenting: removing) { server in
                    Button("Remove", role: .destructive) { bridge.perform { try await bridge.forgetServer(eid: server.eid) } }
                } message: { _ in Text("It’s no longer shared with you. It comes back if it’s shared again.") }
        } header: { Text("Transfer Servers") } footer: {
            Text("Friends’ always-on computers that hold what you send until people are back online.")
        }
        if !bridge.servers.isEmpty { notifications }
    }
    /// Pushes from servers holding messages for you (APNs is wired by the engine;
    /// until this iPhone has a token the section just says so, calmly).
    private var notifications: some View {
        Section {
            Toggle(isOn: Binding(get: { bridge.pushStatus.previews }, set: { on in
                bridge.perform { try await bridge.setPushPreviews(on) }
            })) {
                RowLabel(title: "Show Message Text", symbol: "text.bubble.fill", color: .blue)
            }
            .tint(.green)
            .id("serverNotifications")
        } header: { Text("Notifications from Servers") } footer: {
            Text(pushFooter)
        }
        .task { await bridge.refreshPushStatus() }
        .onReceive(NotificationCenter.default.publisher(for: Notification.Name("DropBeam.mailbox://servers"))) { _ in
            Task { await bridge.refreshPushStatus() }
        }
    }
    private var pushFooter: String {
        let status = bridge.pushStatus
        if !status.enabled {
            return "Notifications turn on once DropBeam can reach Apple’s push service. Until then, held messages arrive when you open the app."
        }
        let text = status.previews ? "Notifications show who it’s from and what they said." : "Notifications only say who it’s from."
        return status.servers > 0 ? "A server holding your messages can let you know when one arrives. \(text)"
            : "Turn on Hold My Messages Here for a server so it can let you know when something arrives."
    }
    private func row(_ server: UsableServer) -> some View {
        HStack(spacing: 14) {
            RowIcon(symbol: "server.rack", color: server.revoked || server.paused || !(server.useIt || server.own) ? .gray : .beam)
            VStack(alignment: .leading, spacing: 2) {
                Text(server.name).foregroundStyle(server.revoked ? .secondary : .primary).lineLimit(1)
                    .alignmentGuide(.listRowSeparatorLeading) { $0[.leading] }
                Text(server.status).font(.subheadline).foregroundStyle(.secondary).lineLimit(2)
            }
            Spacer(minLength: 8)
            if server.revoked {
                Button("Remove") { removing = server }.buttonStyle(.borderless).foregroundStyle(.red)
            } else if !server.own {
                Toggle("Use \(server.name)", isOn: Binding(get: { server.useIt }, set: { on in
                    bridge.perform { try await bridge.setServerPrefs(eid: server.eid, useIt: on) }
                })).labelsHidden().tint(.green).disabled(server.paused)
            }
        }
        .padding(.vertical, 2)
        .accessibilityElement(children: .combine)
    }
}
