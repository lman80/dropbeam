import SwiftUI
import CoreImage.CIFilterBuiltins
import UIKit

struct SendView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var code = ""
    @State private var picking = false
    @State private var receiving = false
    @State private var scanning = false
    @State private var bounce = 0
    @FocusState private var codeFocused: Bool
    private var finished: [Transfer] { bridge.sendTransfers.filter { !$0.active } }
    var body: some View {
        NavigationStack {
            List {
                Section { hero }.clearRow(EdgeInsets(top: 0, leading: 20, bottom: 8, trailing: 20))
                Section {
                    receiveRow
                } header: { Text("Have a Code?") } footer: {
                    Text("Paste or scan any DropBeam code: files someone sent you, a friend’s code, a shared-folder invite or a device link.")
                }
                Section {
                    if bridge.sendTransfers.isEmpty { emptyState.clearRow() }
                    ForEach(bridge.sendTransfers) { transfer in TransferRow(transfer: transfer) }
                } header: {
                    HStack {
                        Text("Transfers")
                        Spacer()
                        if !finished.isEmpty {
                            Button("Clear") { clearFinished() }.font(.body).textCase(nil).accessibilityLabel("Clear finished transfers")
                        }
                    }
                }.headerProminence(.increased)
            }
            .beamList()
            .navigationTitle("Send")
            .animation(.smooth, value: bridge.sendTransfers.map(\.id))
            .toolbar {
                ToolbarItem(placement: .topBarTrailing) {
                    Button { scanning = true; Haptics.tap() } label: { Image(systemName: "qrcode.viewfinder") }
                        .accessibilityLabel("Scan a code")
                }
            }
        }
        .sheet(isPresented: $scanning) {
            QRScannerSheet(title: "Scan a Code", autoSubmit: true, hint: "Scan any DropBeam QR code — files to receive, a friend, a shared folder or one of your devices.") { value in
                try await bridge.openAnyCode(value.trimmingCharacters(in: .whitespacesAndNewlines))
            }
        }
    }
    private var hero: some View {
        VStack(alignment: .leading, spacing: 18) {
            HStack(alignment: .top, spacing: 12) {
                VStack(alignment: .leading, spacing: 6) {
                    Text("Send something good.").font(.title2.weight(.bold))
                    Text("Straight to their device — across the room or across the world.").font(.subheadline).foregroundStyle(.secondary)
                }
                Spacer(minLength: 8)
                Image(systemName: "paperplane.fill").font(.largeTitle).foregroundStyle(.tint)
                    .symbolEffect(.bounce, value: bounce).accessibilityHidden(true)
            }
            GlassGroup {
                ViewThatFits(in: .horizontal) {
                    HStack(spacing: 14) { pickButtons }
                    VStack(spacing: 14) { pickButtons }
                }
            }
        }.padding(.top, 4)
    }
    @ViewBuilder private var pickButtons: some View {
        pickButton("Photos", symbol: "photo.on.rectangle.angled", source: "photos")
        pickButton("Files", symbol: "folder", source: "files")
    }
    private func pickButton(_ title: String, symbol: String, source: String) -> some View {
        Button {
            picking = true; bounce += 1
            bridge.perform { defer { picking = false }; try await bridge.pickAndSend(source: source) }
        } label: {
            VStack(spacing: 10) { Image(systemName: symbol).font(.title); Text(title).font(.headline) }
                .frame(maxWidth: .infinity, minHeight: 88)
        }
        .beamButton(prominent: source == "photos").disabled(picking)
        .accessibilityLabel("Send \(title)")
    }
    private var trimmed: String { code.trimmingCharacters(in: .whitespacesAndNewlines) }
    private var receiveRow: some View {
        HStack(spacing: 12) {
            Image(systemName: "arrow.down.circle.fill").font(.title2).foregroundStyle(.tint).accessibilityHidden(true)
            TextField("Paste a code", text: $code)
                .font(code.isEmpty ? .body : .body.monospaced()).textInputAutocapitalization(.never)
                .autocorrectionDisabled().submitLabel(.go).onSubmit(receive).focused($codeFocused)
                .accessibilityLabel("DropBeam code")
            if trimmed.isEmpty {
                // PasteButton: no "Allow Paste" prompt, the tap itself is consent.
                PasteButton(payloadType: String.self) { strings in
                    Task { @MainActor in code = strings.first?.trimmingCharacters(in: .whitespacesAndNewlines) ?? "" }
                }.labelStyle(.iconOnly).buttonBorderShape(.circle).tint(.beam)
            } else {
                Button(receiving ? "Opening…" : "Go", action: receive)
                    .beamButton().controlSize(.small).disabled(receiving)
            }
        }.frame(minHeight: 44)
    }
    private func receive() {
        guard !trimmed.isEmpty, !receiving else { return }
        receiving = true; codeFocused = false
        let value = trimmed
        bridge.perform { defer { receiving = false }; try await bridge.openAnyCode(value); code = "" }
    }
    private func clearFinished() {
        Haptics.tap()
        let ids = finished.map(\.id)
        Task { for id in ids { try? await bridge.action("dismissTransfer", ["id": id]) } }
    }
    private var emptyState: some View {
        VStack(spacing: 10) {
            ZStack {
                Circle().fill(Color.beam.opacity(0.08)).frame(width: 104, height: 104)
                Image(systemName: "circle.dotted").font(.system(size: 88, weight: .ultraLight)).foregroundStyle(Color.beam.opacity(0.3))
                Image(systemName: "paperplane").font(.system(size: 38, weight: .light)).foregroundStyle(.tint).rotationEffect(.degrees(-12))
                Image(systemName: "sparkle").font(.body).foregroundStyle(.tint).offset(x: 38, y: -30)
            }.accessibilityHidden(true)
            Text("Nothing in Flight").font(.headline)
            Text("Pick a photo or file and make someone’s day.").font(.subheadline).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }.frame(maxWidth: .infinity).padding(.vertical, 12).accessibilityElement(children: .combine)
    }
}

/// One transfer as a List row: file, who, state; progress while moving; the one
/// action that matters for its state (code, accept, share, retry). Swipe and
/// long-press offer the same actions.
struct TransferRow: View {
    @EnvironmentObject private var bridge: Bridge
    let transfer: Transfer
    @State private var copied = false
    private var failed: Bool { transfer.state == "failed" }
    private var canRetry: Bool { (failed || transfer.state == "paused") && (transfer.direction == "send" || transfer.code?.isEmpty == false) }
    private var canShare: Bool { transfer.state == "completed" && transfer.sharePaths?.isEmpty == false }
    private var moving: Bool { transfer.active && transfer.state != "waitingForPeer" && transfer.state != "waitingForAccept" }
    /// Pausing keeps every byte already delivered; only sends we drive can pause.
    private var canPause: Bool { transfer.direction == "send" && ["starting", "waitingForPeer", "connecting", "transferring"].contains(transfer.state ?? "") }
    private var canVerify: Bool { transfer.direction == "send" && transfer.state == "completed" && transfer.verify?.state != "running" }
    var body: some View {
        VStack(alignment: .leading, spacing: 12) {
            HStack(alignment: .center, spacing: 12) {
                icon
                VStack(alignment: .leading, spacing: 3) {
                    Text(transfer.title).font(.headline).lineLimit(2)
                    Text(subtitle).font(.subheadline).foregroundStyle(failed ? Color.red : .secondary).lineLimit(2)
                }
                Spacer(minLength: 4)
                trailing
            }
            if moving {
                VStack(alignment: .leading, spacing: 6) {
                    ProgressView(value: min(100, max(0, transfer.percent ?? 0)), total: 100).tint(.beam)
                    HStack(alignment: .firstTextBaseline, spacing: 8) {
                        Text(progressLine).font(.caption).foregroundStyle(.secondary).monospacedDigit()
                        Spacer(minLength: 4)
                        if let route = transfer.routeLabel { RouteBadge(label: route) }
                    }
                }.accessibilityElement(children: .combine)
            }
            if let detail = transfer.detail, !detail.isEmpty, transfer.active {
                // Parked by "Wait for a Direct Link": say why, and offer the escape hatch.
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) { ProgressView().controlSize(.small); Text(detail).font(.footnote).foregroundStyle(.secondary) }
                    if transfer.state == "waitingForPeer" {
                        Button { bridge.perform { try await bridge.action("forceRelay", ["id": transfer.id]) } } label: { Label("Send over Relay Now", systemImage: "cloud.fill") }
                            .beamButton().controlSize(.small)
                    }
                }
            }
            if transfer.state == "completed" { completedDetails }
            if failed || transfer.state == "paused", let error = transfer.error {
                Text(error).font(.footnote).foregroundStyle(.secondary)
            }
            if transfer.state == "completed" && transfer.sharePaths?.isEmpty != false {
                Text("The files are no longer available to share.").font(.footnote).foregroundStyle(.secondary)
            }
            if transfer.state == "waitingForPeer", let code = transfer.code { codeBlock(code) }
            if transfer.state == "waitingForAccept", transfer.direction == "receive" {
                HStack(spacing: 12) {
                    Button { respond(true) } label: { Text("Accept").frame(maxWidth: .infinity) }.beamButton(prominent: true)
                    Button(role: .destructive) { respond(false) } label: { Text("Decline").frame(maxWidth: .infinity) }.beamButton()
                }.controlSize(.large)
            }
        }
        .padding(.vertical, 6)
        .swipeActions(edge: .trailing, allowsFullSwipe: !transfer.active) {
            if transfer.active {
                Button(role: .destructive) { cancel() } label: { Label("Cancel", systemImage: "xmark") }
            } else {
                Button(role: .destructive) { dismiss() } label: { Label("Remove", systemImage: "trash") }
            }
        }
        .swipeActions(edge: .leading) {
            if canShare { Button { share() } label: { Label("Share", systemImage: "square.and.arrow.up") }.tint(.beam) }
            if canRetry { Button { retry() } label: { Label(transfer.state == "paused" ? "Resume" : "Retry", systemImage: transfer.state == "paused" ? "play.fill" : "arrow.clockwise") }.tint(.orange) }
            if canPause { Button { pause() } label: { Label("Pause", systemImage: "pause.fill") }.tint(.orange) }
        }
        .contextMenu {
            if canShare { Button("Share", systemImage: "square.and.arrow.up", action: share) }
            if canRetry { Button(transfer.state == "paused" ? "Resume" : "Retry", systemImage: "arrow.clockwise", action: retry) }
            if canPause { Button("Pause", systemImage: "pause", action: pause) }
            if canVerify { Button("Verify Copy", systemImage: "checkmark.shield", action: verify) }
            if let code = transfer.code, transfer.state == "waitingForPeer" { Button("Copy Code", systemImage: "doc.on.doc") { UIPasteboard.general.string = code; Haptics.tap() } }
            if transfer.active { Button("Cancel Transfer", systemImage: "xmark", role: .destructive, action: cancel) }
            else { Button("Remove", systemImage: "trash", role: .destructive, action: dismiss) }
        }
    }
    private var subtitle: String {
        let who = transfer.friendName ?? transfer.peer
        let status = transfer.state == "waitingForPeer" && transfer.direction == "send" ? "Share this code to send" : transfer.status
        return [who, status].compactMap { $0?.isEmpty == false ? $0 : nil }.joined(separator: " · ")
    }
    private var progressLine: String {
        var parts = ["\(Formatters.bytes(transfer.bytesDone)) of \(Formatters.bytes(transfer.bytesTotal))"]
        if (transfer.speedBps ?? 0) > 0 { parts.append(Formatters.speed(transfer.speedBps, megabits: bridge.settings?.showMegabits == true)) }
        if let eta = Formatters.eta(transfer.etaSeconds) { parts.append(eta) }
        return parts.joined(separator: " · ")
    }
    private var icon: some View {
        FileGlyph(name: transfer.fileNames?.first ?? "", symbol: (transfer.fileCount ?? 0) > 1 ? "doc.on.doc" : nil)
            .overlay(alignment: .bottomTrailing) {
                Image(systemName: badge.0).font(.system(size: 16, weight: .bold)).symbolRenderingMode(.palette)
                    .foregroundStyle(.white, badge.1)
                    .background(Circle().fill(Color(uiColor: .secondarySystemGroupedBackground)).padding(-2))
                    .offset(x: 5, y: 5).accessibilityHidden(true)
            }
    }
    private var badge: (String, Color) {
        switch transfer.state {
        case "completed": return ("checkmark.circle.fill", .green)
        case "failed": return ("exclamationmark.circle.fill", .red)
        case "paused": return ("pause.circle.fill", .orange)
        case "canceled": return ("xmark.circle.fill", .gray)
        default: return transfer.direction == "receive" ? ("arrow.down.circle.fill", .beam) : ("arrow.up.circle.fill", .beam)
        }
    }
    @ViewBuilder private var trailing: some View {
        if transfer.active {
            if canPause {
                Button(action: pause) { Image(systemName: "pause.circle.fill").font(.title2).symbolRenderingMode(.hierarchical).foregroundStyle(.tint) }
                    .buttonStyle(.borderless).frame(minWidth: 44, minHeight: 44).accessibilityLabel("Pause \(transfer.title)")
            }
            Button(action: cancel) { Image(systemName: "xmark.circle.fill").font(.title2).symbolRenderingMode(.hierarchical).foregroundStyle(.secondary) }
                .buttonStyle(.borderless).frame(minWidth: 44, minHeight: 44).accessibilityLabel("Cancel \(transfer.title)")
        } else if canShare {
            Button(action: share) { Image(systemName: "square.and.arrow.up").frame(width: 20, height: 24) }
                .beamButton().buttonBorderShape(.circle).accessibilityLabel("Share \(transfer.title)")
        } else if canRetry {
            Button(action: retry) { Image(systemName: transfer.state == "paused" ? "play.fill" : "arrow.clockwise").frame(width: 20, height: 24) }
                .beamButton().buttonBorderShape(.circle).accessibilityLabel(transfer.state == "paused" ? "Resume \(transfer.title)" : "Retry \(transfer.title)")
        }
    }
    private func codeBlock(_ code: String) -> some View {
        VStack(spacing: 12) {
            QRCodeView(code: code)
            Text(code).font(.callout.monospaced()).textSelection(.enabled).multilineTextAlignment(.center).lineLimit(3)
            HStack(spacing: 12) {
                Button { UIPasteboard.general.string = code; copied = true; Haptics.success() } label: {
                    Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc").frame(maxWidth: .infinity)
                }.beamButton()
                ShareLink(item: code) { Label("Share", systemImage: "square.and.arrow.up").frame(maxWidth: .infinity) }.beamButton()
            }
        }.frame(maxWidth: .infinity).padding(.top, 4)
    }
    /// Route, end-to-end integrity and "Verify copy" for a finished transfer.
    @ViewBuilder private var completedDetails: some View {
        let rows = transfer.integrity ?? []
        if transfer.routeLabel != nil || !rows.isEmpty {
            HStack(spacing: 8) {
                if let route = transfer.routeLabel { RouteBadge(label: route) }
                if transfer.integrityVerified {
                    Label("Verified end to end", systemImage: "checkmark.seal.fill").font(.caption.weight(.medium)).foregroundStyle(.green)
                } else if !rows.isEmpty {
                    Label("\(rows.filter { !$0.verified }.count) unverified", systemImage: "exclamationmark.triangle.fill").font(.caption.weight(.medium)).foregroundStyle(.orange)
                }
            }
        }
        if !rows.isEmpty {
            DisclosureGroup {
                ForEach(rows) { row in
                    VStack(alignment: .leading, spacing: 2) {
                        Label(row.name, systemImage: row.verified ? "checkmark.circle.fill" : "exclamationmark.circle.fill")
                            .font(.footnote).foregroundStyle(row.verified ? Color.primary : Color.orange).lineLimit(1)
                        if let digest = row.digest, !digest.isEmpty {
                            Text("\((row.algorithm ?? "sha256").uppercased()) \(digest.prefix(16))…").font(.caption2.monospaced()).foregroundStyle(.secondary).textSelection(.enabled)
                        }
                    }.accessibilityElement(children: .combine)
                }
            } label: { Text("Integrity details").font(.footnote.weight(.semibold)) }
        }
        if transfer.direction == "send" { verifySection }
    }
    @ViewBuilder private var verifySection: some View {
        switch transfer.verify?.state ?? "" {
        case "running":
            let report = transfer.verify!
            VStack(alignment: .leading, spacing: 6) {
                HStack {
                    Text("Verifying… \(report.checked) of \(report.total) files").font(.footnote).foregroundStyle(.secondary).monospacedDigit()
                    Spacer()
                    Button("Cancel") { bridge.perform { try await bridge.action("cancelVerify", ["id": transfer.id]) } }.font(.footnote).buttonStyle(.borderless)
                }
                ProgressView(value: Double(report.checked), total: Double(max(1, report.total))).tint(.beam)
            }
        case "done" where (transfer.verify?.mismatched.isEmpty ?? true) && (transfer.verify?.missing.isEmpty ?? true):
            Label("All \(transfer.verify?.total ?? 0) files identical on both devices", systemImage: "checkmark.shield.fill").font(.footnote.weight(.medium)).foregroundStyle(.green)
        case "done":
            let report = transfer.verify!
            DisclosureGroup {
                ForEach(report.mismatched, id: \.self) { Text("Different: \($0)").font(.caption) }
                ForEach(report.missing, id: \.self) { Text("Missing: \($0)").font(.caption) }
            } label: { Label("\(report.mismatched.count + report.missing.count) of \(report.total) files don’t match", systemImage: "xmark.shield.fill").font(.footnote.weight(.semibold)).foregroundStyle(.red) }
            Button(action: verify) { Label("Verify Again", systemImage: "arrow.clockwise") }.beamButton().controlSize(.small)
        default:
            if transfer.verify?.state == "failed" { Text(transfer.verify?.error ?? "Couldn’t verify the copy.").font(.footnote).foregroundStyle(.red) }
            Button(action: verify) { Label("Verify Copy", systemImage: "checkmark.shield") }.beamButton().controlSize(.small)
        }
    }
    private func pause() { bridge.perform { try await bridge.action("pauseTransfer", ["id": transfer.id]) } }
    private func verify() { bridge.perform { try await bridge.action("verifyTransfer", ["id": transfer.id]) } }
    private func cancel() { bridge.perform { try await bridge.cancelTransfer(id: transfer.id) } }
    private func dismiss() { Haptics.tap(); Task { try? await bridge.action("dismissTransfer", ["id": transfer.id]) } }
    private func retry() { bridge.perform { try await bridge.retryTransfer(id: transfer.id) } }
    private func share() { bridge.perform { try await bridge.shareFiles(paths: transfer.sharePaths ?? []) } }
    private func respond(_ accept: Bool) { bridge.perform { try await bridge.respondToOffer(id: transfer.id, accept: accept) } }
}

struct QRCodeView: View {
    let code: String
    var side: CGFloat = 176
    @State private var image: UIImage?
    var body: some View {
        Group {
            if let image { Image(uiImage: image).interpolation(.none).resizable().scaledToFit().padding(12).background(.white, in: RoundedRectangle(cornerRadius: 16, style: .continuous)) }
            else { ProgressView() }
        }.frame(width: side, height: side).accessibilityLabel("QR code for this code")
            .task(id: code) {
                let filter = CIFilter.qrCodeGenerator()
                filter.message = Data(code.utf8)
                filter.correctionLevel = "M"
                if let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 6, y: 6)),
                   let cg = CIContext().createCGImage(output, from: output.extent) { image = UIImage(cgImage: cg) }
            }
    }
}

/// Local / Direct / Relay capsule (the desktop path badge).
struct RouteBadge: View {
    let label: String
    private var color: Color { label.hasPrefix("Local") ? .green : label.hasPrefix("Direct") ? .blue : .orange }
    private var symbol: String { label.hasPrefix("Local") ? "wifi" : label.hasPrefix("Direct") ? "arrow.left.arrow.right" : "cloud.fill" }
    var body: some View {
        Label(label, systemImage: symbol).font(.caption2.weight(.semibold)).foregroundStyle(color)
            .padding(.horizontal, 8).padding(.vertical, 3).background(color.opacity(0.14), in: Capsule())
            .lineLimit(1).accessibilityLabel("Route: \(label)")
    }
}
