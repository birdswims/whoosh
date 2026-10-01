import AppKit
import SwiftUI
import WhooshUI

@main
enum RenderIcon {
    static func main() {
        let app = NSApplication.shared
        app.setActivationPolicy(.prohibited)
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                do {
                    try render()
                    exit(0)
                } catch {
                    fputs("\(error)\n", stderr)
                    exit(1)
                }
            }
        }
        app.run()
    }

    @MainActor
    static func render() throws {
        let arguments = CommandLine.arguments
        guard arguments.count == 2 else {
            throw RenderError("usage: RenderIcon <output-directory>")
        }
        let root = URL(fileURLWithPath: arguments[1], isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        guard let icon = image(AppIconArtwork(), width: 1024, height: 1024, scale: 1) else {
            throw RenderError("could not draw the app icon")
        }
        let iconset = root.appendingPathComponent("AppIcon.iconset", isDirectory: true)
        try FileManager.default.createDirectory(at: iconset, withIntermediateDirectories: true)
        let sizes = [
            ("icon_16x16.png", 16),
            ("icon_16x16@2x.png", 32),
            ("icon_32x32.png", 32),
            ("icon_32x32@2x.png", 64),
            ("icon_128x128.png", 128),
            ("icon_128x128@2x.png", 256),
            ("icon_256x256.png", 256),
            ("icon_256x256@2x.png", 512),
            ("icon_512x512.png", 512),
            ("icon_512x512@2x.png", 1024),
        ]
        for (name, pixels) in sizes {
            guard let data = pngData(from: icon, width: pixels, height: pixels) else {
                throw RenderError("could not scale \(name)")
            }
            try data.write(to: iconset.appendingPathComponent(name))
        }
        guard let logo = image(LogoLockup(), width: 1024, height: 1024, scale: 1),
              let logoPNG = pngData(from: logo, width: logo.width, height: logo.height) else {
            throw RenderError("could not draw the logo")
        }
        try logoPNG.write(to: root.appendingPathComponent("logo.png"))
        guard let backdrop = image(InstallBackdrop(), width: 720, height: 460, scale: 2),
              let backdropPNG = pngData(from: backdrop, width: backdrop.width, height: backdrop.height) else {
            throw RenderError("could not draw the disk image background")
        }
        try backdropPNG.write(to: root.appendingPathComponent("background.png"))
    }

    @MainActor
    static func image<V: View>(_ view: V, width: CGFloat, height: CGFloat, scale: CGFloat) -> CGImage? {
        let renderer = ImageRenderer(content: view.frame(width: width, height: height))
        renderer.scale = scale
        renderer.proposedSize = ProposedViewSize(width: width, height: height)
        renderer.isOpaque = false
        return renderer.cgImage
    }

    static func pngData(from image: CGImage, width: Int, height: Int) -> Data? {
        let source: CGImage
        if image.width == width && image.height == height {
            source = image
        } else {
            guard let context = CGContext(
                data: nil,
                width: width,
                height: height,
                bitsPerComponent: 8,
                bytesPerRow: 0,
                space: CGColorSpaceCreateDeviceRGB(),
                bitmapInfo: CGImageAlphaInfo.premultipliedLast.rawValue
            ) else { return nil }
            context.interpolationQuality = .high
            context.draw(image, in: CGRect(x: 0, y: 0, width: width, height: height))
            guard let scaled = context.makeImage() else { return nil }
            source = scaled
        }
        let rep = NSBitmapImageRep(cgImage: source)
        return rep.representation(using: .png, properties: [:])
    }
}

struct RenderError: Error, CustomStringConvertible {
    var description: String
    init(_ description: String) { self.description = description }
}
