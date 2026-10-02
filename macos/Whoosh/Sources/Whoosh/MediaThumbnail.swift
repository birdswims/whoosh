import AppKit
import AVFoundation
import ImageIO
import UniformTypeIdentifiers

/// Small photo and video previews for the send list.
/// Still-image orientation follows the file transform, so portraits stay upright.
enum MediaThumbnail {
    enum Kind: Equatable {
        case photo
        case video
    }

    static let side: CGFloat = 64

    static func kind(of path: String) -> Kind? {
        let ext = URL(fileURLWithPath: path).pathExtension.lowercased()
        guard !ext.isEmpty else { return nil }
        if let type = UTType(filenameExtension: ext) {
            if type.conforms(to: .image) { return .photo }
            if type.conforms(to: .movie) || type.conforms(to: .video) { return .video }
        }
        switch ext {
        case "jpg", "jpeg", "png", "gif", "heic", "heif", "webp", "tif", "tiff", "avif", "bmp", "dng":
            return .photo
        case "mp4", "mov", "m4v", "webm", "mkv", "avi":
            return .video
        default:
            return nil
        }
    }

    static func make(path: String, scale: CGFloat) -> CGImage? {
        guard let kind = kind(of: path) else { return nil }
        let key = cacheKey(path, scale: scale)
        if let cached = cached(key) { return cached }
        let image: CGImage?
        switch kind {
        case .photo:
            image = photo(path: path, scale: scale)
        case .video:
            image = video(path: path, scale: scale)
        }
        if let image { store(image, for: key) }
        return image
    }

    static func thumbnail(_ image: CGImage) -> NSImage {
        NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
    }

    private static func photo(path: String, scale: CGFloat) -> CGImage? {
        let url = URL(fileURLWithPath: path) as CFURL
        guard let source = CGImageSourceCreateWithURL(url, [kCGImageSourceShouldCache: false] as CFDictionary) else {
            return nil
        }
        let options: [CFString: Any] = [
            kCGImageSourceCreateThumbnailFromImageAlways: true,
            kCGImageSourceCreateThumbnailWithTransform: true,
            kCGImageSourceThumbnailMaxPixelSize: maxPixel(scale),
            kCGImageSourceShouldCacheImmediately: true,
        ]
        return CGImageSourceCreateThumbnailAtIndex(source, 0, options as CFDictionary)
    }

    /// The opening frame of a phone video is often black, so this samples a little way in.
    private static func video(path: String, scale: CGFloat) -> CGImage? {
        let asset = AVURLAsset(url: URL(fileURLWithPath: path))
        let generator = AVAssetImageGenerator(asset: asset)
        generator.appliesPreferredTrackTransform = true
        let pixels = CGFloat(maxPixel(scale))
        generator.maximumSize = CGSize(width: pixels, height: pixels)
        generator.requestedTimeToleranceBefore = CMTime(seconds: 1, preferredTimescale: 600)
        generator.requestedTimeToleranceAfter = CMTime(seconds: 1, preferredTimescale: 600)
        if let image = frame(generator, at: 0.4) {
            return image
        }
        return frame(generator, at: 0)
    }

    private static func frame(_ generator: AVAssetImageGenerator, at seconds: Double) -> CGImage? {
        var actual = CMTime.zero
        let time = CMTime(seconds: seconds, preferredTimescale: 600)
        return try? generator.copyCGImage(at: time, actualTime: &actual)
    }

    private static func maxPixel(_ scale: CGFloat) -> Int {
        Int((side * max(scale, 1)).rounded(.up))
    }

    private static func cacheKey(_ path: String, scale: CGFloat) -> String {
        let url = URL(fileURLWithPath: path)
        let values = try? url.resourceValues(forKeys: [.contentModificationDateKey, .fileSizeKey])
        let modified = values?.contentModificationDate?.timeIntervalSince1970 ?? 0
        let size = values?.fileSize ?? -1
        return "\(path)\n\(size)\n\(modified)\n\(scale)"
    }

    private static let cacheLock = NSLock()
    private static var cache: [String: CGImage] = [:]

    private static func cached(_ key: String) -> CGImage? {
        cacheLock.lock()
        defer { cacheLock.unlock() }
        return cache[key]
    }

    private static func store(_ image: CGImage, for key: String) {
        cacheLock.lock()
        if cache.count > 600 {
            cache.removeAll()
        }
        cache[key] = image
        cacheLock.unlock()
    }
}
