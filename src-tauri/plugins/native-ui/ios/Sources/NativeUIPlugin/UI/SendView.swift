import SwiftUI
import CoreImage.CIFilterBuiltins
import UIKit

struct SendView: View {
    @EnvironmentObject private var bridge: Bridge
    @State private var code = ""
    @State private var picking = false
    @State private var receiving = false
    @State private var bounce = 0
    var body: some View {
        NavigationStack {
            ScrollView {
                VStack(alignment: .leading, spacing: 24) {
                    GlassCard {
                        VStack(alignment: .leading, spacing: 22) {
                            HStack(alignment: .top) {
                                VStack(alignment: .leading, spacing: 8) {
                                    Text("Send something good.").font(.title2.weight(.semibold))
                                    Text("Across the room. Across the world.").font(.body).foregroundStyle(.secondary)
                                }
                                Spacer(minLength: 8)
                                Image(systemName: "paperplane.fill").font(.largeTitle).foregroundStyle(.tint)
                                    .symbolEffect(.bounce, value: bounce)
                            }
                            GlassGroup {
                                ViewThatFits(in: .horizontal) {
                                    HStack(spacing: 14) { pickButtons }
                                    VStack(spacing: 14) { pickButtons }
                                }
                            }
                        }.padding(.vertical, 4)
                    }
                    GlassCard {
                        VStack(alignment: .leading, spacing: 12) {
                            Text("Have a code?").font(.headline)
                            ViewThatFits(in: .horizontal) {
                                HStack(spacing: 12) { receiveField; receiveButton }
                                VStack(alignment: .leading, spacing: 12) { receiveField; receiveButton }
                            }
                        }
                    }
                    Text("Transfers").font(.title2.weight(.semibold))
                    if bridge.sendTransfers.isEmpty { emptyState }
                    else {
                        GlassGroup {
                            LazyVStack(spacing: 16) {
                                ForEach(bridge.sendTransfers) { transfer in TransferCard(transfer: transfer) }
                            }
                        }
                    }
                }.padding(20)
            }.contentMargins(.bottom, 24, for: .scrollContent)
                .navigationTitle("Send").beamCanvas()
        }
    }
    @ViewBuilder private var pickButtons: some View {
        pickButton("Photos", symbol: "photo.on.rectangle", source: "photos")
        pickButton("Files", symbol: "folder", source: "files")
    }
    private func pickButton(_ title: String, symbol: String, source: String) -> some View {
        Button {
            picking = true; bounce += 1
            bridge.perform { defer { picking = false }; try await bridge.pickAndSend(source: source) }
        } label: {
            VStack(spacing: 12) { Image(systemName: symbol).font(.title); Text(title).font(.headline) }
                .frame(maxWidth: .infinity, minHeight: 90)
        }.beamButton(prominent: source == "photos").disabled(picking)
    }
    private var receiveField: some View {
        TextField("", text: $code, prompt: Text("Paste a receive code").font(.body))
            .font(code.isEmpty ? .body : .body.monospaced()).textInputAutocapitalization(.never)
            .autocorrectionDisabled().submitLabel(.go).onSubmit(receive)
            .padding(.vertical, 10).accessibilityLabel("Receive code")
    }
    private var receiveButton: some View {
        Button(receiving ? "Receiving…" : "Receive", action: receive).beamButton(prominent: true)
            .disabled(receiving || code.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty)
    }
    private func receive() {
        let trimmed = code.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, !receiving else { return }
        receiving = true
        bridge.perform { defer { receiving = false }; try await bridge.receiveWithCode(code: trimmed); code = "" }
    }
    private var emptyState: some View {
        VStack(spacing: 14) {
            ZStack {
                Circle().fill(Color.beam.opacity(0.07)).frame(width: 132, height: 132)
                Image(systemName: "circle.dotted").font(.system(size: 110, weight: .ultraLight)).foregroundStyle(Color.beam.opacity(0.3))
                Image(systemName: "paperplane").font(.system(size: 48, weight: .light)).foregroundStyle(.tint).rotationEffect(.degrees(-12))
                Image(systemName: "sparkle").font(.title3).foregroundStyle(.tint).offset(x: 48, y: -38)
            }.accessibilityHidden(true)
            Text("Nothing in flight").font(.title2.weight(.semibold))
            Text("Pick a photo or file and make someone’s day.").font(.body).foregroundStyle(.secondary).multilineTextAlignment(.center)
        }.frame(maxWidth: .infinity).padding(.vertical, 22)
    }
}

struct TransferCard: View {
    @EnvironmentObject private var bridge: Bridge
    let transfer: Transfer
    @State private var copied = false
    var body: some View {
        GlassCard {
            VStack(alignment: .leading, spacing: 16) {
                HStack(alignment: .top, spacing: 14) {
                    Image(systemName: transfer.state == "completed" ? "checkmark.seal.fill" : Formatters.symbol(transfer.fileNames?.first))
                        .font(.title).foregroundStyle(transfer.state == "completed" ? Color.green : .beam)
                        .frame(width: 42, height: 48).accessibilityHidden(true)
                    VStack(alignment: .leading, spacing: 5) {
                        Text(transfer.title).font(.headline).lineLimit(3)
                        Text([transfer.friendName ?? transfer.peer, transfer.status].compactMap { $0 }.joined(separator: " · "))
                            .font(.footnote).foregroundStyle(.secondary)
                    }
                    Spacer(minLength: 0)
                    if transfer.active {
                        Button { bridge.perform { try await bridge.cancelTransfer(id: transfer.id) } } label: {
                            Image(systemName: "xmark").frame(width: 24, height: 32)
                        }.beamButton().buttonBorderShape(.circle).accessibilityLabel("Cancel \(transfer.title)")
                    }
                }
                if transfer.active {
                    ProgressView(value: min(100, max(0, transfer.percent ?? 0)), total: 100).tint(.beam)
                    Text("\(Formatters.bytes(transfer.bytesDone)) of \(Formatters.bytes(transfer.bytesTotal)) · \(Formatters.speed(transfer.speedBps, megabits: bridge.settings?.showMegabits == true))")
                        .font(.footnote).foregroundStyle(.secondary)
                    if let eta = Formatters.eta(transfer.etaSeconds) { Text(eta).font(.footnote).foregroundStyle(.secondary) }
                }
                if transfer.state == "waitingForPeer", let code = transfer.code {
                    VStack(spacing: 14) {
                        QRCodeView(code: code)
                        Text(code).font(.body.monospaced()).textSelection(.enabled).multilineTextAlignment(.center)
                        Button { UIPasteboard.general.string = code; copied = true; Haptics.tap() } label: {
                            Label(copied ? "Copied" : "Copy Code", systemImage: copied ? "checkmark" : "doc.on.doc")
                        }.beamButton()
                    }.frame(maxWidth: .infinity)
                }
                if transfer.state == "waitingForAccept", transfer.direction == "receive" {
                    HStack {
                        Button("Accept Files") { bridge.perform { try await bridge.respondToOffer(id: transfer.id, accept: true) } }.beamButton(prominent: true)
                        Button("Decline", role: .destructive) { bridge.perform { try await bridge.respondToOffer(id: transfer.id, accept: false) } }.beamButton()
                    }
                }
                if transfer.state == "completed" {
                    Button { bridge.perform { try await bridge.shareFiles(paths: transfer.sharePaths ?? []) } } label: { Label("Share", systemImage: "square.and.arrow.up") }
                        .beamButton().disabled(transfer.sharePaths?.isEmpty != false)
                    if transfer.sharePaths?.isEmpty != false { Text("The original files are no longer available to share.").font(.footnote).foregroundStyle(.secondary) }
                }
                if transfer.state == "failed" || transfer.state == "paused" {
                    if let error = transfer.error { Text(error).font(.footnote).foregroundStyle(.secondary) }
                    Button { bridge.perform { try await bridge.retryTransfer(id: transfer.id) } } label: { Label("Retry", systemImage: "arrow.clockwise") }.beamButton(prominent: true)
                }
            }
        }
    }
}
struct QRCodeView: View {
    let code: String
    @State private var image: UIImage?
    var body: some View {
        Group {
            if let image { Image(uiImage: image).interpolation(.none).resizable().scaledToFit().padding(12).background(.white, in: RoundedRectangle(cornerRadius: 16)) }
        }.frame(maxWidth: 180, maxHeight: 180).accessibilityLabel("Receive code QR")
            .task(id: code) {
                let filter = CIFilter.qrCodeGenerator()
                filter.message = Data(code.utf8)
                filter.correctionLevel = "M"
                if let output = filter.outputImage?.transformed(by: CGAffineTransform(scaleX: 6, y: 6)),
                   let cg = CIContext().createCGImage(output, from: output.extent) { image = UIImage(cgImage: cg) }
            }
    }
}
