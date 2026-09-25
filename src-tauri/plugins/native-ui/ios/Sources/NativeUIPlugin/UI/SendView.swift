import SwiftUI
import CoreImage.CIFilterBuiltins
import UIKit

struct SendView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var code = ""
    @State private var picking = false
    @State private var receiving = false
    @State private var scanning = false
    @FocusState private var codeFocused: Bool
    private var finished: [Transfer] { bridge.sendTransfers.filter { !$0.active } }
    var body: some View {
        NavigationStack {
            List {
                Section { hero }.clearRow(EdgeInsets(top: 4, leading: 20, bottom: 4, trailing: 20))
                Section {
                    receiveRow
                } header: { Text("Receive") } footer: {
                    Text("Paste or scan a code from DropBeam — files, a friend, a shared folder or one of your devices.")
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
        ActionTileRow {
            pickButton("Photos", symbol: "photo.on.rectangle", source: "photos")
            pickButton("Files", symbol: "doc", source: "files")
            SendFolderButton()
        }
    }
    private func pickButton(_ title: String, symbol: String, source: String) -> some View {
        ActionTile(title: title, symbol: symbol, large: true) {
            picking = true
            bridge.perform { defer { picking = false }; try await bridge.pickAndSend(source: source) }
        }
        .disabled(picking)
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
        Text("Files you send and receive show up here.")
            .font(.subheadline).foregroundStyle(.secondary)
            .frame(maxWidth: .infinity).padding(.vertical, 20)
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
        VStack(alignment: .leading, spacing: 10) {
            HStack(alignment: .center, spacing: 12) {
                icon
                VStack(alignment: .leading, spacing: 2) {
                    Text(transfer.title).font(.body.weight(.semibold)).lineLimit(2).truncationMode(.middle)
                    Text(subtitle).font(.subheadline).foregroundStyle(failed ? Color.red : .secondary).lineLimit(2)
                        .accessibilityLabel("\(transfer.status), \(subtitle)")
                }
                Spacer(minLength: 4)
                trailing
            }
            if moving {
                VStack(alignment: .leading, spacing: 5) {
                    ProgressView(value: min(100, max(0, transfer.percent ?? 0)), total: 100).tint(.beam)
                    Text(progressLine).font(.caption).foregroundStyle(.secondary).monospacedDigit().lineLimit(1)
                }.accessibilityElement(children: .combine)
            }
            if let detail = transfer.detail, !detail.isEmpty, transfer.active {
                // Parked by "Wait for a Direct Link": say why, and offer the escape hatch.
                VStack(alignment: .leading, spacing: 8) {
                    HStack(spacing: 8) { ProgressView().controlSize(.small); Text(detail).font(.footnote).foregroundStyle(.secondary) }
                    if transfer.state == "waitingForPeer" {
                        Button("Send Through a Relay Now") { bridge.perform { try await bridge.action("forceRelay", ["id": transfer.id]) } }
                            .font(.footnote.weight(.semibold)).buttonStyle(.borderless)
                    }
                }
            }
            if transfer.state == "completed" { completedDetails }
            if failed || transfer.state == "paused", let error = transfer.error {
                Text(error).font(.footnote).foregroundStyle(.secondary)
            }
            if transfer.state == "completed" && transfer.sharePaths?.isEmpty != false && transfer.direction == "receive" {
                Text("No longer on this iPhone.").font(.footnote).foregroundStyle(.secondary)
            }
            if transfer.state == "waitingForPeer", let code = transfer.code { codeBlock(code) }
            if transfer.state == "waitingForAccept", transfer.direction == "receive" {
                HStack(spacing: 12) {
                    Button(role: .destructive) { respond(false) } label: { Text("Decline").frame(maxWidth: .infinity) }.beamButton()
                    Button { respond(true) } label: { Text("Accept").frame(maxWidth: .infinity) }.beamButton(prominent: true)
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
        let name = transfer.friendName.flatMap { $0.isEmpty ? nil : $0 } ?? humanPeer(transfer.peer)
        let who = name.map { (transfer.direction == "receive" ? "From " : "To ") + $0 }
        let status: String
        switch transfer.state {
        case "waitingForPeer" where transfer.direction == "send" && transfer.code != nil: status = "Waiting for someone to receive"
        case "failed": status = transfer.direction == "send" ? "Couldn’t send" : "Couldn’t receive"
        default: status = transfer.status
        }
        // A finished row's badge already says Sent/Received; the line says who and how much.
        if transfer.state == "completed" {
            let size = (transfer.bytesTotal ?? 0) > 0 ? Formatters.bytes(transfer.bytesTotal) : nil
            return [who ?? status, size].compactMap { $0 }.joined(separator: " · ")
        }
        return [who, status].compactMap { $0 }.joined(separator: " · ")
    }
    private var progressLine: String {
        var parts = ["\(Formatters.bytes(transfer.bytesDone)) of \(Formatters.bytes(transfer.bytesTotal))"]
        if (transfer.speedBps ?? 0) > 0 { parts.append(Formatters.speed(transfer.speedBps, megabits: bridge.settings?.showMegabits == true)) }
        if let eta = Formatters.eta(transfer.etaSeconds) { parts.append(eta) }
        // The route only matters to people when it explains a slow transfer.
        if transfer.routeLabel?.hasPrefix("Relay") == true { parts.append("via relay") }
        return parts.joined(separator: " · ")
    }
    private var icon: some View {
        Group {
            if (transfer.fileCount ?? 1) <= 1, let path = transfer.sharePaths?.first.map(LocalPaths.resolve), LocalMedia(path: path) != nil {
                MediaThumbnail(path: path, width: 44, height: 44, badges: false).clipShape(RoundedRectangle(cornerRadius: 12, style: .continuous))
            } else {
                FileGlyph(name: transfer.fileNames?.first ?? "", symbol: (transfer.fileCount ?? 0) > 1 ? "doc.on.doc" : nil)
            }
        }
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
            QRCodeView(code: code, side: 168)
            CodeLine(code: code).padding(.horizontal, 8)
            HStack(spacing: 12) {
                Button { UIPasteboard.general.string = code; copied = true; Haptics.success() } label: {
                    Label(copied ? "Copied" : "Copy", systemImage: copied ? "checkmark" : "doc.on.doc").frame(maxWidth: .infinity)
                }.beamButton()
                ShareLink(item: code) { Label("Share", systemImage: "square.and.arrow.up").frame(maxWidth: .infinity) }.beamButton()
            }
        }.frame(maxWidth: .infinity).padding(.top, 4)
    }
    /// A finished transfer stays quiet unless something needs attention: files the
    /// end-to-end check couldn't confirm, or a "Verify Copy" the user started.
    @ViewBuilder private var completedDetails: some View {
        let unverified = (transfer.integrity ?? []).filter { !$0.verified }
        if !unverified.isEmpty {
            DisclosureGroup {
                ForEach(unverified) { row in
                    Text(row.name).font(.footnote).foregroundStyle(.secondary).lineLimit(1).truncationMode(.middle)
                }
            } label: {
                Label(unverified.count == 1 ? "1 file couldn’t be checked" : "\(unverified.count) files couldn’t be checked", systemImage: "exclamationmark.triangle.fill")
                    .font(.footnote.weight(.medium)).foregroundStyle(.orange)
            }
        }
        if transfer.direction == "send", transfer.verify != nil { verifySection }
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
            Button("Verify Again", action: verify).font(.footnote.weight(.semibold)).buttonStyle(.borderless)
        default:
            HStack {
                Text(transfer.verify?.error ?? "Couldn’t verify the copy.").font(.footnote).foregroundStyle(.red)
                Spacer(minLength: 8)
                Button("Try Again", action: verify).font(.footnote.weight(.semibold)).buttonStyle(.borderless)
            }
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
