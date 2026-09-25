import SwiftUI
import ImageIO
import AVFoundation

/// The actor's executor owns all media I/O and decoding, never the main actor.
/// A path + pixel-size cache bounds memory even for a 14-asset Photos selection.
actor ThumbnailProvider {
    static let shared = ThumbnailProvider()
    final class Preview {
        let image: UIImage
        let duration: Double?
        init(_ image: UIImage, duration: Double? = nil) { self.image = image; self.duration = duration }
    }
    // NSCache is thread-safe; `cached` peeks it synchronously so views can draw a
    // cached image on their very first frame instead of flashing a placeholder.
    nonisolated(unsafe) private let cache = NSCache<NSString, Preview>()
    init() { cache.totalCostLimit = 48 * 1024 * 1024; cache.countLimit = 160 }
    private static func key(_ path: String, _ points: CGFloat) -> NSString {
        "\(path)|\(max(1, Int(ceil(points * 2))))" as NSString
    }
    nonisolated func cached(path: String, points: CGFloat) -> Preview? { cache.object(forKey: Self.key(path, points)) }

    func image(path: String, points: CGFloat, fullSize: Bool = false) async -> Preview? {
        let pixels = max(1, Int(ceil(points * 2)))
        let key = fullSize ? "\(path)|full" as NSString : Self.key(path, points)
        if let cached = cache.object(forKey: key) { return cached }
        let url = ChatAttachment.fileURL(path)
        let preview: Preview?
        if LocalMedia(path: path)?.video == true {
            let asset = AVURLAsset(url: url)
            let generator = AVAssetImageGenerator(asset: asset)
            generator.appliesPreferredTrackTransform = true
            generator.maximumSize = CGSize(width: pixels, height: pixels)
            guard let frame = try? await generator.image(at: .zero) else { return nil }
            let duration = try? await asset.load(.duration).seconds
            preview = Preview(UIImage(cgImage: frame.image), duration: duration)
        } else {
            let source: CGImageSource?
            if path.hasPrefix("https://"), let remote = URL(string: path),
               let (data, _) = try? await URLSession.shared.data(from: remote) {
                source = CGImageSourceCreateWithData(data as CFData, [kCGImageSourceShouldCache: false] as CFDictionary)
            } else {
                source = CGImageSourceCreateWithURL(url as CFURL, [kCGImageSourceShouldCache: false] as CFDictionary)
            }
            guard let source else { return nil }
            assert(!Thread.isMainThread, "Media must decode off the main thread")
            // Even the viewer applies EXIF orientation; only its active page may
            // request original dimensions. Thumbnails never decode a full image.
            let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any]
            let original = max(properties?[kCGImagePropertyPixelWidth] as? Int ?? pixels,
                               properties?[kCGImagePropertyPixelHeight] as? Int ?? pixels)
            guard let cg = CGImageSourceCreateThumbnailAtIndex(source, 0, [
                kCGImageSourceCreateThumbnailFromImageAlways: true,
                kCGImageSourceCreateThumbnailWithTransform: true,
                kCGImageSourceShouldCacheImmediately: true,
                kCGImageSourceThumbnailMaxPixelSize: fullSize ? original : pixels
            ] as CFDictionary) else { return nil }
            preview = Preview(UIImage(cgImage: cg))
        }
        if let preview, !fullSize {
            let cg = preview.image.cgImage
            cache.setObject(preview, forKey: key, cost: (cg?.bytesPerRow ?? 0) * (cg?.height ?? 0))
        }
        return preview
    }
}

struct MediaThumbnail: View {
    let path: String
    let width: CGFloat
    let height: CGFloat
    var badges = true
    @State private var preview: ThumbnailProvider.Preview?
    /// The path `preview` belongs to: a new size keeps showing the old image until the
    /// sharper one is ready, so re-layouts never flash the placeholder.
    @State private var shownPath: String?
    init(path: String, width: CGFloat, height: CGFloat, badges: Bool = true) {
        self.path = path; self.width = width; self.height = height; self.badges = badges
        let hit = ThumbnailProvider.shared.cached(path: path, points: max(width, height))
        _preview = State(initialValue: hit)
        _shownPath = State(initialValue: hit == nil ? nil : path)
    }
    var body: some View {
        ZStack {
            Color(uiColor: .secondarySystemFill)
            if let preview { Image(uiImage: preview.image).resizable().scaledToFill() }
            else { Image(systemName: Formatters.symbol(path)).foregroundStyle(.secondary) }
        }
        .frame(width: width, height: height).clipped()
        .overlay {
            if badges && preview != nil && LocalMedia(path: path)?.video == true {
                Image(systemName: "play.fill").font(.system(size: min(22, max(12, min(width, height) * 0.16))))
                    .foregroundStyle(.white).padding(min(14, max(6, min(width, height) * 0.1)))
                    .background(.black.opacity(0.35), in: Circle())
            }
        }
        .overlay(alignment: .bottomTrailing) {
            if badges, let seconds = preview?.duration, seconds.isFinite, seconds >= 0, min(width, height) >= 60 {
                Text(String(format: "%d:%02d", Int(seconds) / 60, Int(seconds) % 60))
                    .font(.caption2.monospacedDigit().weight(.medium)).foregroundStyle(.white)
                    .padding(.horizontal, 6).padding(.vertical, 2)
                    .background(.black.opacity(0.55), in: Capsule()).padding(6)
            }
        }
        .task(id: "\(path)|\(Int(width))|\(Int(height))") {
            if shownPath != path { preview = nil }
            let result = await ThumbnailProvider.shared.image(path: path, points: max(width, height))
            if !Task.isCancelled, let result { preview = result; shownPath = path }
        }
    }
}
