import SwiftUI
import AVKit

struct ChatAttachment: View {
    @EnvironmentObject private var bridge: Bridge
    let message: ChatMessage
    @State private var selected: LocalMedia?
    private var transfer: Transfer? { bridge.transfers.first { $0.chatOnly == true && $0.id == message.fileXferId } ?? bridge.transfers.first { $0.id == message.fileXferId } }
    private var paths: [String] { Self.availablePaths(message, bridge: bridge) }
    private var failed: Bool { message.fileXferFailed == true || ["failed", "canceled"].contains(transfer?.state ?? "") }
    private var active: Bool { transfer?.active == true }
    private struct Item: Identifiable {
        let id: Int
        let name: String
        let path: String?
    }
    private var items: [Item] {
        var remaining = paths
        return (message.files ?? []).enumerated().map { index, name in
            let match = remaining.firstIndex { Self.fileURL($0).lastPathComponent == name }
            let path = match.map { remaining.remove(at: $0) }
            return Item(id: index, name: name, path: path)
        }
    }
    private var media: [Item] { items.filter { LocalMedia(path: $0.name) != nil } }
    private var documents: [Item] { items.filter { LocalMedia(path: $0.name) == nil } }
    private var availableMedia: [LocalMedia] { media.compactMap { $0.path.flatMap(LocalMedia.init) } }
    var body: some View {
        VStack(alignment: .leading, spacing: 4) {
            if !media.isEmpty { grid.clipShape(RoundedRectangle(cornerRadius: 18)) }
            ForEach(documents) { item in
                Button {
                    if let path = item.path { bridge.perform { try await bridge.openChatFile(path: path) } }
                } label: {
                    HStack(spacing: 8) {
                        Image(systemName: Formatters.symbol(item.name)).font(.title3)
                        Text(item.name).font(.subheadline).lineLimit(1).truncationMode(.middle)
                        Spacer(minLength: 0)
                        Image(systemName: item.path == nil ? "clock" : "arrow.down.circle").font(.caption)
                    }.padding(10).background(Color(uiColor: .systemGray5), in: RoundedRectangle(cornerRadius: 12))
                }.buttonStyle(.plain).disabled(item.path == nil)
            }
            if failed {
                Button(message.fromMe ? "Not Delivered · Retry" : "Not Delivered · Ask sender to retry") {
                    if message.fromMe { bridge.perform { try await bridge.retryChatFile(friendId: message.peerId, messageId: message.id) } }
                }.font(.caption).foregroundStyle(.secondary).disabled(!message.fromMe)
            } else if active {
                ProgressView(value: min(1, max(0, (transfer?.percent ?? 0) / 100))).tint(.beam)
                Text("\(message.fromMe ? "Sending" : "Receiving") \(Int(transfer?.percent ?? 0))%")
                    .font(.caption).foregroundStyle(.secondary)
            } else if items.contains(where: { $0.path == nil }) {
                Text("Waiting for files…").font(.caption).foregroundStyle(.secondary)
            }
        }.frame(maxWidth: 240, alignment: .leading)
        .fullScreenCover(item: $selected) { item in
            PagedMediaViewer(items: availableMedia, initialPath: item.path).environmentObject(bridge)
        }
    }
    private var grid: some View {
        Group {
            if media.count == 1 { tile(media[0]) }
            else if media.count == 2 {
                HStack(spacing: 2) { tile(media[0]); tile(media[1]) }
            } else if media.count == 3 {
                HStack(spacing: 2) {
                    tile(media[0])
                    VStack(spacing: 2) { tile(media[1]); tile(media[2]) }
                }
            } else {
                VStack(spacing: 2) {
                    HStack(spacing: 2) { tile(media[0]); tile(media[1]) }
                    HStack(spacing: 2) { tile(media[2]); tile(media[3], extra: media.count - 4) }
                }
            }
        }.aspectRatio(media.count == 2 ? 2 : 1, contentMode: .fit)
    }
    private func tile(_ item: Item, extra: Int = 0) -> some View {
        GeometryReader { geo in
            Button {
                if let path = item.path { selected = LocalMedia(path: path) }
            } label: {
                MediaThumbnail(path: item.path ?? item.name, width: geo.size.width, height: geo.size.height)
                    .overlay {
                        if extra > 0 { Color.black.opacity(0.4); Text("+\(extra)").font(.title.weight(.semibold)).foregroundStyle(.white) }
                    }
            }.buttonStyle(.plain).disabled(item.path == nil).accessibilityLabel(item.name)
        }
    }
    nonisolated static func fileURL(_ path: String) -> URL { path.hasPrefix("file://") ? URL(string: path) ?? URL(fileURLWithPath: path) : URL(fileURLWithPath: path) }
    @MainActor static func availablePaths(_ message: ChatMessage, bridge: Bridge) -> [String] {
        let transfer = bridge.transfers.first { $0.chatOnly == true && $0.id == message.fileXferId } ?? bridge.transfers.first { $0.id == message.fileXferId }
        // Use the engine's explicit completed manifest; filenames alone cannot
        // prove arrival, and duplicate leaf names must stay independently openable.
        let completed = transfer?.chatTransfer?.completedPaths ?? [:]
        var paths = completed.sorted { $0.key.localizedStandardCompare($1.key) == .orderedAscending }.map(\.value)
        if message.fromMe { paths += transfer?.sharePaths ?? [] }
        if let path = message.path, message.fromMe || transfer == nil || transfer?.state == "completed" {
            paths.insert(path, at: 0)
        }
        var seen = Set<String>()
        return paths.filter { seen.insert($0).inserted }
    }
}

struct PagedMediaViewer: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let items: [LocalMedia]
    let initialPath: String
    @State private var selection = ""
    var body: some View {
        NavigationStack {
            TabView(selection: $selection) {
                ForEach(items) { item in
                    MediaPage(item: item, active: selection == item.path).tag(item.path)
                }
            }.tabViewStyle(.page(indexDisplayMode: items.count > 1 ? .always : .never))
                .background(.black).navigationTitle(items.first { $0.path == selection }?.name ?? "Media")
                .navigationBarTitleDisplayMode(.inline)
                .toolbar {
                    ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
                    ToolbarItem(placement: .topBarTrailing) {
                        Button { bridge.perform { try await bridge.shareFiles(paths: [selection]) } } label: {
                            Image(systemName: "square.and.arrow.up").frame(width: 44, height: 44)
                        }.accessibilityLabel("Share").disabled(selection.isEmpty)
                    }
                }
                .onAppear { selection = initialPath }
        }.tint(.beam).preferredColorScheme(.dark)
    }
}

struct MediaViewer: View {
    let path: String
    let name: String
    let video: Bool
    var body: some View {
        if let item = LocalMedia(path: path) { PagedMediaViewer(items: [item], initialPath: path) }
    }
}
private struct MediaPage: View {
    let item: LocalMedia
    let active: Bool
    @State private var player: AVPlayer?
    @State private var image: UIImage?
    @State private var unavailable = false
    var body: some View {
        Group {
            if item.video { NativeVideoPlayer(player: player) }
            else if let image { ImageViewer(image: image) }
            else if unavailable { Text("This image is no longer available.").foregroundStyle(.white) }
            else { ProgressView().tint(.white) }
        }
        .task(id: active) {
            player?.pause(); player = nil; image = nil; unavailable = false
            guard active else { return }
            if item.video { player = AVPlayer(url: ChatAttachment.fileURL(item.path)); player?.play() }
            else {
                let preview = await ThumbnailProvider.shared.image(path: item.path, points: 400, fullSize: true)
                if !Task.isCancelled { image = preview?.image; unavailable = preview == nil }
            }
        }
        .onDisappear { player?.pause(); player = nil; image = nil }
    }
}

struct ImageViewer: UIViewRepresentable {
    let image: UIImage
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeUIView(context: Context) -> UIScrollView {
        let scroll = ImageScrollView()
        scroll.delegate = context.coordinator
        scroll.minimumZoomScale = 1; scroll.maximumZoomScale = 5
        scroll.showsVerticalScrollIndicator = false; scroll.showsHorizontalScrollIndicator = false
        scroll.imageView.image = image
        scroll.imageView.contentMode = .scaleAspectFit
        scroll.addSubview(scroll.imageView)
        context.coordinator.image = scroll.imageView
        return scroll
    }
    func updateUIView(_ scroll: UIScrollView, context: Context) { context.coordinator.image?.image = image }
    final class Coordinator: NSObject, UIScrollViewDelegate {
        weak var image: UIImageView?
        func viewForZooming(in scrollView: UIScrollView) -> UIView? { image }
    }
    final class ImageScrollView: UIScrollView {
        let imageView = UIImageView()
        private var previousSize = CGSize.zero
        override func layoutSubviews() {
            super.layoutSubviews()
            if bounds.size != previousSize {
                previousSize = bounds.size
                setZoomScale(1, animated: false)
                imageView.frame = CGRect(origin: .zero, size: bounds.size)
                contentSize = bounds.size
            }
        }
    }
}

struct NativeVideoPlayer: UIViewControllerRepresentable {
    let player: AVPlayer?
    func makeUIViewController(context: Context) -> AVPlayerViewController { let controller = AVPlayerViewController(); controller.player = player; return controller }
    func updateUIViewController(_ controller: AVPlayerViewController, context: Context) { controller.player = player }
    static func dismantleUIViewController(_ controller: AVPlayerViewController, coordinator: ()) { controller.player?.pause(); controller.player = nil }
}
struct LocalMedia: Identifiable {
    let path: String
    let name: String
    let video: Bool
    var id: String { path }
    init?(path: String) {
        let ext = (path as NSString).pathExtension.lowercased()
        let video = ["mp4", "mov", "m4v"].contains(ext)
        guard video || ["jpg", "jpeg", "png", "heic", "heif", "gif", "webp", "tiff", "tif", "bmp", "avif"].contains(ext) else { return nil }
        self.path = path; self.name = (path as NSString).lastPathComponent; self.video = video
    }
}
