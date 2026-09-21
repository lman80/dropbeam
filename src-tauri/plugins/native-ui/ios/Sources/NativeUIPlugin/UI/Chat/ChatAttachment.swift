import SwiftUI
import AVKit
import ImageIO

struct ChatAttachment: View {
    @EnvironmentObject private var bridge: Bridge
    let message: ChatMessage
    @State private var thumbnail: UIImage?
    @State private var viewer = false
    @State private var choosingFile = false
    private var transfer: Transfer? { bridge.transfers.first { $0.chatOnly == true && $0.id == message.fileXferId } }
    private var name: String { message.files?.first ?? "Attachment" }
    private var path: String? { Self.availablePaths(message, bridge: bridge).first }
    private var fileExtension: String { (name as NSString).pathExtension.lowercased() }
    private var isImage: Bool { ["jpg", "jpeg", "png", "heic", "heif", "gif", "webp", "tiff"].contains(fileExtension) }
    private var isVideo: Bool { ["mov", "mp4", "m4v"].contains(fileExtension) }
    private var failed: Bool { message.fileXferFailed == true || ["failed", "canceled"].contains(transfer?.state ?? "") }
    private var progress: Double { min(1, max(0, (transfer?.percent ?? 0) / 100)) }
    private var active: Bool { !failed && (transfer?.active == true || (transfer == nil && message.fileXferId != nil && path == nil)) }
    private var status: String {
        if failed { return message.fromMe ? "Failed · Tap to retry" : "Failed · Ask sender to retry" }
        if active { return "\(message.fromMe ? "Sending" : "Receiving") \(Int(progress * 100))%" }
        if transfer?.state == "paused" { return "Paused" }
        if transfer?.state == "completed" || path != nil { return message.fromMe ? "Sent" : "Received" }
        return "Waiting for files…"
    }
    var body: some View {
        Button(action: open) {
            VStack(alignment: .leading, spacing: 0) {
                if let thumbnail, isImage || isVideo {
                    Image(uiImage: thumbnail).resizable().scaledToFit().frame(maxWidth: 240, maxHeight: 280)
                        .overlay {
                            if isVideo { Image(systemName: "play.fill").font(.title).foregroundStyle(.white).padding(16).background(.ultraThinMaterial, in: Circle()) }
                        }
                        .overlay(alignment: .bottom) {
                            if active || failed {
                                HStack { Text(status).font(.caption); Spacer(); if active { progressRing } }
                                    .foregroundStyle(.primary).padding(8).background(.regularMaterial)
                            }
                        }
                } else {
                    HStack(spacing: 10) {
                        Image(systemName: Formatters.symbol(name)).font(.system(size: 28)).frame(width: 36, height: 44)
                        VStack(alignment: .leading, spacing: 4) {
                            Text((message.files?.count ?? 0) > 1 ? "\(message.files!.count) files" : name).font(.subheadline.weight(.semibold)).lineLimit(1).truncationMode(.middle)
                            Text("\(Formatters.bytes(message.bytes)) · \(status)").font(.caption).opacity(0.8).fixedSize(horizontal: false, vertical: true)
                        }
                        if active { progressRing }
                    }.padding(12).frame(maxWidth: 240, alignment: .leading)
                    if active { ProgressView(value: progress).tint(message.fromMe ? .white : .beam).frame(height: 2).padding(.horizontal, 12).padding(.bottom, 8) }
                }
            }
        }.buttonStyle(.plain).frame(minHeight: 44).accessibilityLabel("\(name), \(status)")
            .task(id: path) {
                thumbnail = nil
                guard let path else { return }
                if isImage {
                    thumbnail = await Task.detached(priority: .utility) { Self.imageThumbnail(path) }.value
                } else if isVideo {
                    let generator = AVAssetImageGenerator(asset: AVURLAsset(url: Self.fileURL(path)))
                    generator.appliesPreferredTrackTransform = true
                    generator.maximumSize = CGSize(width: 720, height: 720)
                    if let result = try? await generator.image(at: .zero) { thumbnail = UIImage(cgImage: result.image) }
                }
            }
            .fullScreenCover(isPresented: $viewer) {
                if let path {
                    MediaViewer(path: path, name: name, video: isVideo).environmentObject(bridge)
                }
            }
            .confirmationDialog("Files", isPresented: $choosingFile, titleVisibility: .visible) {
                ForEach(Self.availablePaths(message, bridge: bridge), id: \.self) { path in
                    Button(Self.fileURL(path).lastPathComponent) { bridge.perform { try await bridge.openChatFile(path: path) } }
                }
                Button("Share All") { bridge.perform { try await bridge.shareFiles(paths: Self.availablePaths(message, bridge: bridge)) } }
            }
    }
    private var progressRing: some View {
        ZStack {
            Circle().stroke(.secondary.opacity(0.2), lineWidth: 2)
            Circle().trim(from: 0, to: progress).stroke(message.fromMe ? Color.white : .beam, style: StrokeStyle(lineWidth: 2, lineCap: .round)).rotationEffect(.degrees(-90))
        }.frame(width: 22, height: 22).accessibilityLabel("\(Int(progress * 100)) percent")
    }
    private func open() {
        if failed && message.fromMe { bridge.perform { try await bridge.retryChatFile(friendId: message.peerId, messageId: message.id) }; return }
        guard let path else { return }
        Haptics.tap()
        if (message.files?.count ?? 0) > 1 { choosingFile = true }
        else if isImage || isVideo { viewer = true }
        else { bridge.perform { try await bridge.openChatFile(path: path) } }
    }
    nonisolated static func fileURL(_ path: String) -> URL { path.hasPrefix("file://") ? URL(string: path) ?? URL(fileURLWithPath: path) : URL(fileURLWithPath: path) }
    @MainActor static func availablePaths(_ message: ChatMessage, bridge: Bridge) -> [String] {
        let transfer = bridge.transfers.first { $0.chatOnly == true && $0.id == message.fileXferId }
        // Use the engine's explicit completed manifest; filenames alone cannot
        // prove arrival, and duplicate leaf names must stay independently openable.
        let completed = transfer?.chatTransfer?.completedPaths ?? [:]
        var paths = completed.sorted { $0.key < $1.key }.map(\.value)
        if message.fromMe { paths += transfer?.sharePaths ?? [] }
        if let path = message.path, message.fromMe || transfer == nil || transfer?.state == "completed" {
            paths.insert(path, at: 0)
        }
        var seen = Set<String>()
        return paths.filter { seen.insert($0).inserted && FileManager.default.fileExists(atPath: fileURL($0).path) }
    }
    nonisolated private static func imageThumbnail(_ path: String) -> UIImage? {
        guard let source = CGImageSourceCreateWithURL(fileURL(path) as CFURL, nil),
              let image = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceThumbnailMaxPixelSize: 720,
                kCGImageSourceCreateThumbnailWithTransform: true
              ] as CFDictionary) else { return nil }
        return UIImage(cgImage: image)
    }
}

struct MediaViewer: View {
    @EnvironmentObject private var bridge: Bridge
    @Environment(\.dismiss) private var dismiss
    let path: String
    let name: String
    let video: Bool
    @State private var player: AVPlayer?
    var body: some View {
        NavigationStack {
            Group {
                if video { NativeVideoPlayer(player: player) }
                else { ImageViewer(path: path) }
            }
            .background(.black).navigationTitle(name).navigationBarTitleDisplayMode(.inline)
            .toolbar {
                ToolbarItem(placement: .cancellationAction) { Button("Done") { dismiss() } }
                ToolbarItem(placement: .topBarTrailing) {
                    Button { bridge.perform { try await bridge.shareFiles(paths: [path]) } } label: {
                        Image(systemName: "square.and.arrow.up").frame(width: 44, height: 44)
                    }.accessibilityLabel("Share")
                }
            }
            .onAppear { if video { player = AVPlayer(url: ChatAttachment.fileURL(path)); player?.play() } }
            .onDisappear { player?.pause(); player = nil }
        }.tint(.beam)
    }
}

struct ImageViewer: UIViewRepresentable {
    let path: String
    func makeCoordinator() -> Coordinator { Coordinator() }
    func makeUIView(context: Context) -> UIScrollView {
        let scroll = ImageScrollView()
        scroll.delegate = context.coordinator
        scroll.minimumZoomScale = 1; scroll.maximumZoomScale = 5
        scroll.showsVerticalScrollIndicator = false; scroll.showsHorizontalScrollIndicator = false
        scroll.imageView.image = UIImage(contentsOfFile: ChatAttachment.fileURL(path).path)
        scroll.imageView.contentMode = .scaleAspectFit
        scroll.addSubview(scroll.imageView)
        context.coordinator.image = scroll.imageView
        return scroll
    }
    func updateUIView(_ scroll: UIScrollView, context: Context) {}
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
        guard video || ["jpg", "jpeg", "png", "heic", "heif", "gif", "webp", "tiff", "bmp"].contains(ext) else { return nil }
        self.path = path; self.name = (path as NSString).lastPathComponent; self.video = video
    }
}
