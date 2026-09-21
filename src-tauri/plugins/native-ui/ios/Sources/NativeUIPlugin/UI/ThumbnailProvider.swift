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
    private let cache = NSCache<NSString, Preview>()
    init() { cache.totalCostLimit = 48 * 1024 * 1024; cache.countLimit = 160 }

    func image(path: String, points: CGFloat, fullSize: Bool = false) async -> Preview? {
        let pixels = max(1, Int(ceil(points * 2)))
        let key = "\(path)|\(fullSize ? "full" : String(pixels))" as NSString
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
    var body: some View {
        ZStack {
            Color(uiColor: .secondarySystemFill)
            if let preview { Image(uiImage: preview.image).resizable().scaledToFill() }
            else { Image(systemName: Formatters.symbol(path)).foregroundStyle(.secondary) }
        }
        .frame(width: width, height: height).clipped()
        .overlay {
            if badges && LocalMedia(path: path)?.video == true {
                Image(systemName: "play.fill").foregroundStyle(.white).padding(8).background(.black.opacity(0.35), in: Circle())
            }
        }
        .overlay(alignment: .bottomTrailing) {
            if badges, let seconds = preview?.duration, seconds.isFinite, seconds >= 0 {
                Text(String(format: "%d:%02d", Int(seconds) / 60, Int(seconds) % 60))
                    .font(.caption2.monospacedDigit()).foregroundStyle(.white).padding(4)
                    .background(.black.opacity(0.65), in: Capsule()).padding(4)
            }
        }
        .task(id: "\(path)|\(width)|\(height)") {
            preview = nil
            let result = await ThumbnailProvider.shared.image(path: path, points: max(width, height))
            if !Task.isCancelled { preview = result }
        }
    }
}
