import SwiftUI
import AVKit
import QuickLook

/// Open in place (docs/locations-v2.md): video and audio stream from the
/// friend's device through DropBeam's own 127.0.0.1 server (AVPlayer seeks
/// with range requests, nothing is copied first); everything else is fetched
/// into the app cache through the same block cache and shown with Quick Look.
struct StreamOpen: Decodable { let url: String; let kind: String; let mime: String; let size: Double }
struct FetchedFile: Decodable { let path: String }
struct ThumbReply: Decodable { let data: String? }
struct StreamPathStatus: Decodable { let path: String; let bytesPerSec: Double; let rttMs: Double? }

struct OpenInPlaceView: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let friendID: String
    let location: SharedLocation
    let path: String
    let entry: BrowserEntry
    var onDownload: () -> Void = {}
    @State private var player: AVPlayer?
    @State private var file: URL?
    @State private var error: String?
    @State private var status: StreamPathStatus?
    private var args: [String: Any] { ["friendId": friendID, "locationId": location.id, "path": path, "name": entry.name] }
    var body: some View {
        NavigationStack {
            Group {
                if let player {
                    PlayerController(player: player).ignoresSafeArea(edges: .bottom)
                } else if let file {
                    QuickLookController(url: file).ignoresSafeArea(edges: .bottom)
                } else if let error {
                    ContentUnavailableView { Label("Couldn’t Open", systemImage: "exclamationmark.triangle") } description: { Text(error) } actions: {
                        Button("Download Instead") { dismiss(); onDownload() }.beamButton()
                    }
                } else {
                    VStack(spacing: 12) {
                        ProgressView()
                        Text("Opening from \(location.name)…").font(.subheadline).foregroundStyle(.secondary)
                    }
                }
            }
            .navigationTitle(entry.name).navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .topBarLeading) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .topBarTrailing) { Button { dismiss(); onDownload() } label: { Image(systemName: "arrow.down.circle") }.accessibilityLabel("Download") }
            }
            .safeAreaInset(edge: .top) { if let status { badge(status) } }
        }
        .task { await open() }
        .task {
            // Path badge + live speed while the file comes in.
            while !Task.isCancelled {
                if let s: StreamPathStatus = try? await bridge.call("streamStatus", ["friendId": friendID]) { status = s }
                try? await Task.sleep(for: .seconds(1))
            }
        }
        .onDisappear { player?.pause() }
    }
    private func badge(_ s: StreamPathStatus) -> some View {
        let label = s.path == "local" ? "Local network" : s.path == "direct" ? "Direct" : s.path == "relay" ? "Relay" : "Connecting"
        let speed = s.bytesPerSec > 0 ? " · \(Formatters.bytes(s.bytesPerSec))/s" : ""
        return Text(label + speed).font(.caption.weight(.semibold)).monospacedDigit()
            .padding(.horizontal, 10).padding(.vertical, 4).glassCapsule()
            .accessibilityLabel("Streaming over \(label)\(speed)")
    }
    private func open() async {
        do {
            let opened: StreamOpen = try await bridge.call("browserOpen", args)
            if opened.kind == "video" || opened.kind == "audio", let url = URL(string: opened.url) {
                let p = AVPlayer(url: url)
                p.play()
                player = p
            } else {
                let fetched: FetchedFile = try await bridge.call("browserFetch", args)
                file = URL(fileURLWithPath: fetched.path)
            }
        } catch is CancellationError {} catch { self.error = error.localizedDescription }
    }
}

private struct PlayerController: UIViewControllerRepresentable {
    let player: AVPlayer
    func makeUIViewController(context: Context) -> AVPlayerViewController {
        let c = AVPlayerViewController()
        c.player = player
        c.allowsPictureInPicturePlayback = true
        return c
    }
    func updateUIViewController(_ c: AVPlayerViewController, context: Context) { c.player = player }
}

private struct QuickLookController: UIViewControllerRepresentable {
    let url: URL
    func makeCoordinator() -> Source { Source(url: url) }
    func makeUIViewController(context: Context) -> QLPreviewController {
        let c = QLPreviewController()
        c.dataSource = context.coordinator
        return c
    }
    func updateUIViewController(_ c: QLPreviewController, context: Context) {
        if context.coordinator.url != url { context.coordinator.url = url; c.reloadData() }
    }
    final class Source: NSObject, QLPreviewControllerDataSource {
        var url: URL
        init(url: URL) { self.url = url }
        func numberOfPreviewItems(in controller: QLPreviewController) -> Int { 1 }
        func previewController(_ controller: QLPreviewController, previewItemAt index: Int) -> QLPreviewItem { url as NSURL }
    }
}

/// A picture row's small preview, made by the friend's device; the file glyph
/// until (or unless) it arrives.
struct RemoteThumb: View {
    @EnvironmentObject private var bridge: Bridge
    let friendID: String
    let locationID: String
    let path: String
    let entry: BrowserEntry
    let size: CGFloat
    @State private var image: UIImage?
    private static let pictures: Set<String> = ["jpg", "jpeg", "png", "gif", "webp", "bmp"]
    static func applies(to name: String) -> Bool { pictures.contains((name as NSString).pathExtension.lowercased()) }
    var body: some View {
        Group {
            if let image { Image(uiImage: image).resizable().scaledToFill().frame(width: size, height: size).clipShape(RoundedRectangle(cornerRadius: 8)) }
            else { FileGlyph(name: entry.name, size: size) }
        }
        .frame(width: size, height: size)
        .task(id: entry.name) {
            let args: [String: Any] = ["friendId": friendID, "locationId": locationID, "path": path, "name": entry.name, "size": entry.size, "modified": entry.modified]
            guard let reply: ThumbReply = try? await bridge.call("browserThumb", args), let url = reply.data,
                  let comma = url.firstIndex(of: ","), let data = Data(base64Encoded: String(url[url.index(after: comma)...])) else { return }
            image = UIImage(data: data)
        }
    }
}
