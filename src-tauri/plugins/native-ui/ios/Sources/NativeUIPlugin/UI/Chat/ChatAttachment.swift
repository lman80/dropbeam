import SwiftUI
import AVKit

struct ChatAttachment: View {
    @EnvironmentObject private var bridge: Bridge
    let message: ChatMessage
    @State private var selected: LocalMedia?
    /// A lone photo/video keeps its own shape (clamped like Messages) instead of a square crop.
    @State private var singleAspect: CGFloat?
    private var transfer: Transfer? { bridge.transfers.first { $0.chatOnly == true && $0.id == message.fileXferId } ?? bridge.transfers.first { $0.id == message.fileXferId } }
    private var paths: [String] { Self.availablePaths(message, bridge: bridge) }
    private var failed: Bool { message.fileXferFailed == true || ["failed", "canceled"].contains(transfer?.state ?? "") }
    private var active: Bool { transfer?.active == true }
    /// A held send from a friend who needs your OK first (Download / Decline).
    private var pending: PendingFile? { message.fromMe || transfer != nil ? nil : bridge.pendingFiles.first { $0.linkId == message.fileXferId } }
    @State private var deciding = false
    /// This session's answer to a pending file (the engine then updates the message).
    @State private var accepted: Bool?
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
            if !media.isEmpty { grid.clipShape(RoundedRectangle(cornerRadius: 18, style: .continuous)) }
            ForEach(documents) { item in
                Button {
                    if let path = item.path { bridge.perform { try await bridge.openChatFile(path: path) } }
                } label: {
                    HStack(spacing: 10) {
                        Image(systemName: Formatters.symbol(item.name)).font(.title2).frame(width: 30)
                        VStack(alignment: .leading, spacing: 1) {
                            Text(item.name).font(.subheadline.weight(.semibold)).lineLimit(1).truncationMode(.middle)
                            if documents.count == 1 && media.isEmpty, let bytes = message.bytes, bytes > 0 {
                                Text(Formatters.bytes(bytes)).font(.caption).opacity(0.75)
                            }
                        }
                        Spacer(minLength: 0)
                        // Only a file that hasn't arrived yet needs a marker.
                        if item.path == nil { Image(systemName: "clock").font(.body).opacity(0.8).accessibilityLabel("Not yet available") }
                    }
                    .foregroundStyle(message.fromMe ? Color.white : Color.primary)
                    .padding(.horizontal, 12).padding(.vertical, 10)
                    .background(ChatPalette.fill(message.fromMe), in: RoundedRectangle(cornerRadius: 18, style: .continuous))
                }.buttonStyle(.plain).disabled(item.path == nil)
            }
            if failed {
                Button(message.fromMe ? "Not Delivered · Retry" : "Not Delivered · Ask sender to retry") {
                    if message.fromMe { bridge.perform { try await bridge.retryChatFile(friendId: message.peerId, messageId: message.id) } }
                }.font(.caption).foregroundStyle(.secondary).disabled(!message.fromMe)
            } else if transfer?.state == "held" {
                // Short: the thread's status line under the latest message says the rest.
                Label { Text("Held on \(transfer?.heldOn ?? "your Transfer Server")") } icon: { Image(systemName: "server.rack") }
                    .labelStyle(ServerLineStyle())
            } else if active {
                ProgressView(value: min(1, max(0, (transfer?.percent ?? 0) / 100))).tint(ChatPalette.sent)
                Text(transfer?.heldOn.map { "Sending to \($0) · \(Int(transfer?.percent ?? 0))%" } ?? "\(message.fromMe ? "Sending" : "Receiving") \(Int(transfer?.percent ?? 0))%")
                    .font(.caption).foregroundStyle(.secondary)
            } else if let pending {
                Text("Waiting on \(pending.serverName)").font(.caption).foregroundStyle(.secondary)
                HStack(spacing: 8) {
                    Button("Decline") { decide(pending, false) }.beamButton()
                    Button("Download") { decide(pending, true) }.beamButton(prominent: true)
                }
                .controlSize(.small).disabled(deciding).padding(.bottom, 6)
            } else if !message.fromMe, let via = message.via, items.contains(where: { $0.path == nil }) {
                Text(accepted == false ? "Declined" : accepted == true ? "Downloading…" : "On its way from \(via)…").font(.caption).foregroundStyle(.secondary)
            } else if items.contains(where: { $0.path == nil }) {
                Text("Waiting for files…").font(.caption).foregroundStyle(.secondary)
            }
        }.frame(maxWidth: 240, alignment: .leading)
        #if targetEnvironment(simulator)
        // QA: `-openViewer` opens the first multi-photo message on its SECOND photo.
        .task {
            guard CommandLine.arguments.contains("-openViewer"), !Self.qaViewerOpened, availableMedia.count > 1 else { return }
            Self.qaViewerOpened = true
            try? await Task.sleep(for: .seconds(1.5))
            selected = availableMedia[1]
        }
        #endif
        .fullScreenCover(item: $selected) { item in
            PagedMediaViewer(items: availableMedia, initialPath: item.path).environmentObject(bridge)
        }
    }
    private func decide(_ file: PendingFile, _ accept: Bool) {
        deciding = true
        bridge.perform {
            do { try await bridge.decidePendingFile(linkId: file.linkId, accept: accept); accepted = accept }
            catch { deciding = false; throw error }
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
        }.aspectRatio(media.count == 2 ? 2 : media.count == 1 ? (singleAspect ?? media.first?.path.flatMap { Self.aspects[$0] } ?? 1) : 1, contentMode: .fit)
        .task(id: media.count == 1 ? media.first?.path : nil) {
            guard media.count == 1, let path = media.first?.path else { return }
            let preview = await ThumbnailProvider.shared.image(path: path, points: 240)
            guard let size = preview?.image.size, size.width > 0, size.height > 0, !Task.isCancelled else { return }
            let aspect = min(1.78, max(0.66, size.width / size.height))
            Self.aspects[path] = aspect
            if singleAspect != aspect { singleAspect = aspect }
        }
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
    @MainActor private static var qaViewerOpened = false
    /// Remembered shapes so a bubble re-appearing while scrolling never changes height.
    @MainActor private static var aspects: [String: CGFloat] = [:]
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
        return paths.map(LocalPaths.resolve).filter { seen.insert($0).inserted }
    }
}

/// Photos-style viewer for a message's media: opens on the tapped item, swipes
/// between the others, pinch/double-tap to zoom (a zoomed photo pans instead of
/// paging until it's back at 1x), tap to hide the bars.
struct PagedMediaViewer: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let items: [LocalMedia]
    @State private var selection: String
    @State private var chromeHidden = false
    init(items: [LocalMedia], initialPath: String) {
        self.items = items
        // Start ON the tapped page: setting it after appearing made the pager
        // jump from the first photo (or not move at all).
        _selection = State(initialValue: items.contains { $0.path == initialPath } ? initialPath : items.first?.path ?? initialPath)
    }
    private var index: Int? { items.firstIndex { $0.path == selection } }
    var body: some View {
        NavigationStack {
            TabView(selection: $selection) {
                ForEach(items) { item in
                    MediaPage(item: item, active: selection == item.path, toggleChrome: { withAnimation(.easeInOut(duration: 0.2)) { chromeHidden.toggle() } })
                        .tag(item.path)
                }
            }
            .tabViewStyle(.page(indexDisplayMode: .never))
            .background(Color.black.ignoresSafeArea())
            .ignoresSafeArea()
            .navigationTitle(items.count > 1 ? "\((index ?? 0) + 1) of \(items.count)" : (items.first?.name ?? "Photo"))
            .navigationBarTitleDisplayMode(.inline)
            .toolbarBackground(.hidden, for: .navigationBar)
            .toolbar(chromeHidden ? .hidden : .visible, for: .navigationBar)
            .statusBarHidden(chromeHidden)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { bridge.perform { try await bridge.shareFiles(paths: [selection]) } } label: {
                        Image(systemName: "square.and.arrow.up")
                    }.accessibilityLabel("Share").disabled(selection.isEmpty)
                }
            }
            .accessibilityAction(named: "Next") { step(1) }
            .accessibilityAction(named: "Previous") { step(-1) }
            #if targetEnvironment(simulator)
            // QA: `-viewerStep` pages forward after appearing (no touch input in CI).
            .task { if CommandLine.arguments.contains("-viewerStep") { try? await Task.sleep(for: .seconds(2)); withAnimation { step(1) } } }
            #endif
        }.tint(.white).preferredColorScheme(.dark)
    }
    private func step(_ delta: Int) {
        guard let index, items.indices.contains(index + delta) else { return }
        selection = items[index + delta].path
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
    let toggleChrome: () -> Void
    @State private var player: AVPlayer?
    @State private var image: UIImage?
    @State private var unavailable = false
    var body: some View {
        ZStack {
            Color.black
            if item.video {
                NativeVideoPlayer(player: player)
            } else if let image {
                ImageViewer(image: image, onTap: toggleChrome)
            } else if unavailable {
                Text("This image is no longer available.").foregroundStyle(.white.opacity(0.8))
            } else {
                // The cached thumbnail keeps a page from flashing a spinner mid-swipe.
                if let thumb = ThumbnailProvider.shared.cached(path: item.path, points: 240)?.image {
                    Image(uiImage: thumb).resizable().scaledToFit()
                }
                ProgressView().tint(.white)
            }
        }
        // Neighbours load too (the pager builds them just before they slide in), and a
        // page keeps its photo when it scrolls away — no reset, no reload on return.
        .task(id: item.path) {
            guard !item.video, image == nil else { return }
            let preview = await ThumbnailProvider.shared.image(path: item.path, points: 400, fullSize: true)
            if !Task.isCancelled { image = preview?.image; unavailable = preview == nil }
        }
        .task(id: active) {
            guard item.video else { return }
            if active {
                if player == nil { player = AVPlayer(url: ChatAttachment.fileURL(item.path)) }
                player?.play()
            } else { player?.pause() }
        }
        .onDisappear { player?.pause() }
    }
}

struct ImageViewer: UIViewRepresentable {
    let image: UIImage
    var onTap: (() -> Void)? = nil
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeUIView(context: Context) -> ImageScrollView {
        let scroll = ImageScrollView()
        scroll.delegate = context.coordinator
        scroll.minimumZoomScale = 1; scroll.maximumZoomScale = 4
        scroll.showsVerticalScrollIndicator = false; scroll.showsHorizontalScrollIndicator = false
        scroll.contentInsetAdjustmentBehavior = .never
        scroll.decelerationRate = .fast
        scroll.imageView.image = image
        scroll.imageView.contentMode = .scaleAspectFit
        scroll.addSubview(scroll.imageView)
        let double = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.doubleTap(_:)))
        double.numberOfTapsRequired = 2
        scroll.addGestureRecognizer(double)
        let single = UITapGestureRecognizer(target: context.coordinator, action: #selector(Coordinator.singleTap(_:)))
        single.require(toFail: double)
        scroll.addGestureRecognizer(single)
        context.coordinator.scroll = scroll
        return scroll
    }
    func updateUIView(_ scroll: ImageScrollView, context: Context) {
        context.coordinator.onTap = onTap
        if scroll.imageView.image !== image { scroll.imageView.image = image; scroll.setNeedsLayout() }
    }
    final class Coordinator: NSObject, UIScrollViewDelegate {
        weak var scroll: ImageScrollView?
        var onTap: (() -> Void)?
        func viewForZooming(in scrollView: UIScrollView) -> UIView? { (scrollView as? ImageScrollView)?.imageView }
        func scrollViewDidZoom(_ scrollView: UIScrollView) { (scrollView as? ImageScrollView)?.centerImage() }
        @objc func singleTap(_ gesture: UITapGestureRecognizer) { onTap?() }
        /// Photos: double-tap zooms in on that spot, or back out.
        @objc func doubleTap(_ gesture: UITapGestureRecognizer) {
            guard let scroll else { return }
            if scroll.zoomScale > scroll.minimumZoomScale + 0.01 {
                scroll.setZoomScale(scroll.minimumZoomScale, animated: true)
            } else {
                let point = gesture.location(in: scroll.imageView)
                let scale: CGFloat = 2.5
                let size = CGSize(width: scroll.bounds.width / scale, height: scroll.bounds.height / scale)
                scroll.zoom(to: CGRect(x: point.x - size.width / 2, y: point.y - size.height / 2, width: size.width, height: size.height), animated: true)
            }
        }
    }
    /// The image view is sized to the photo's fitted rect (not the whole page), so
    /// at 1x there is nothing to scroll and every horizontal swipe goes to the pager.
    final class ImageScrollView: UIScrollView {
        let imageView = UIImageView()
        private var laidOut = CGSize.zero
        private var imageSize = CGSize.zero
        override func layoutSubviews() {
            super.layoutSubviews()
            let size = imageView.image?.size ?? .zero
            guard bounds.width > 0, bounds.height > 0 else { return }
            if bounds.size != laidOut || size != imageSize {
                laidOut = bounds.size; imageSize = size
                zoomScale = 1
                let fit = size.width > 0 && size.height > 0 ? min(bounds.width / size.width, bounds.height / size.height) : 1
                imageView.frame = CGRect(origin: .zero, size: CGSize(width: size.width * fit, height: size.height * fit))
                contentSize = imageView.frame.size
                centerImage()
                contentOffset = CGPoint(x: -contentInset.left, y: -contentInset.top)
            }
        }
        func centerImage() {
            let x = max(0, (bounds.width - contentSize.width) / 2)
            let y = max(0, (bounds.height - contentSize.height) / 2)
            contentInset = UIEdgeInsets(top: y, left: x, bottom: y, right: x)
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

/// A small caption line with a leading glyph (Transfer Server status under a file).
struct ServerLineStyle: LabelStyle {
    func makeBody(configuration: Configuration) -> some View {
        HStack(alignment: .firstTextBaseline, spacing: 5) {
            configuration.icon.font(.caption2)
            configuration.title.fixedSize(horizontal: false, vertical: true)
        }
        .font(.caption).foregroundStyle(.secondary)
    }
}
