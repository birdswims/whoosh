import AppKit
import SwiftUI
import WhooshUI

/// Draws the interface to PNG files when WHOOSH_SNAPSHOT is set.
/// Used to check layout without a screen recording permission.
enum Snapshot {
    @MainActor
    static func writeIfRequested() {
        guard let directory = ProcessInfo.processInfo.environment["WHOOSH_SNAPSHOT"] else { return }
        do {
            try writeAll(to: directory)
            fputs("snapshots \(directory)\n", stderr)
        } catch {
            fputs("snapshot failed: \(error)\n", stderr)
            exit(1)
        }
        exit(0)
    }

    @MainActor
    private static func writeAll(to directory: String) throws {
        let root = URL(fileURLWithPath: directory, isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try render(RootView().environment(populated()), name: "main-light", scheme: .light, width: 1000, height: 720, to: root)
        try render(RootView().environment(populated()), name: "main-dark", scheme: .dark, width: 1000, height: 720, to: root)
        try render(RootView().environment(emptyModel()), name: "empty-light", scheme: .light, width: 1000, height: 720, to: root)
        try render(RootView().environment(emptyModel()), name: "empty-dark", scheme: .dark, width: 1000, height: 720, to: root)
        try render(RootView().environment(offerModel()), name: "offer-light", scheme: .light, width: 1000, height: 720, to: root)
        try render(RootView().environment(offerModel()), name: "offer-dark", scheme: .dark, width: 1000, height: 720, to: root)
        try render(RootView().environment(trustModel()), name: "trust-light", scheme: .light, width: 1000, height: 720, to: root)
        try render(RootView().environment(stoppedModel()), name: "stopped-light", scheme: .light, width: 1000, height: 720, to: root)
        try render(SettingsView().environment(populated()), name: "settings-light", scheme: .light, width: 460, height: 820, to: root)
        try render(SettingsView().environment(populated()), name: "settings-dark", scheme: .dark, width: 460, height: 820, to: root)
        try render(ActivityView().environment(populated()), name: "activity-light", scheme: .light, width: 480, height: 560, to: root)
        try render(ActivityView().environment(populated()), name: "activity-dark", scheme: .dark, width: 480, height: 560, to: root)
        try render(ActivityView().environment(emptyModel()), name: "activity-empty", scheme: .light, width: 480, height: 560, to: root)
    }

    @MainActor
    private static func render<V: View>(_ view: V, name: String, scheme: ColorScheme, width: CGFloat, height: CGFloat, to root: URL) throws {
        let content = view
            .environment(\.colorScheme, scheme)
            .frame(width: width, height: height)
            .background(Theme.canvas)
        let renderer = ImageRenderer(content: content)
        renderer.scale = 1
        renderer.proposedSize = ProposedViewSize(width: width, height: height)
        renderer.isOpaque = true
        guard let image = renderer.cgImage else {
            throw CocoaError(.fileWriteUnknown)
        }
        let rep = NSBitmapImageRep(cgImage: image)
        guard let data = rep.representation(using: .png, properties: [:]) else {
            throw CocoaError(.fileWriteUnknown)
        }
        try data.write(to: root.appendingPathComponent("\(name).png"))
    }

    private static func base() -> AppModel {
        let model = AppModel()
        model.version = "0.1.0"
        model.deviceName = "Studio"
        model.saveDir = FileManager.default.homeDirectoryForCurrentUser.appendingPathComponent("Whoosh").path
        model.fingerprint = "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12"
        model.visibility = "lan address 192.168.1.20 on en0 (Wi-Fi Home). Put the phone on this Wi-Fi."
        return model
    }

    private static func populated() -> AppModel {
        let model = base()
        model.receiving = true
        model.requirePin = true
        model.pin = "1842"
        model.sawPeers = true
        model.peers = [
            Peer(id: "whoosh|192.168.1.30:45823", via: "whoosh", name: "Office Mac", detail: "ab12 cd34", address: "192.168.1.30:45823", fingerprint: "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12", trusted: true, trustsUs: true),
            Peer(id: "quickshare|192.168.1.40:12345", via: "quickshare", name: "Pixel", detail: "192.168.1.40:12345", address: "192.168.1.40:12345", fingerprint: nil, trusted: false),
            Peer(id: "airdrop|192.168.1.50:8770", via: "airdrop", name: "AirDrop device", detail: "192.168.1.50:8770", address: "192.168.1.50:8770", fingerprint: nil, trusted: false),
        ]
        model.selectedPeerID = model.peers[0].id
        model.trustedDevices = [
            TrustedDevice(
                fingerprint: "ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12cd34ef56ab12",
                name: "Office Mac",
                addresses: ["192.168.1.30:45823"],
                isSelf: false
            ),
        ]
        model.files = [
            SendFile(path: "/tmp/vacation.jpg", name: "vacation.jpg"),
            SendFile(path: "/tmp/clip.mp4", name: "clip.mp4"),
            SendFile(path: "/tmp/notes.txt", name: "notes.txt"),
        ]
        attachPreviews(model)
        model.activity = [
            ActivityItem(id: "1", direction: "in", state: "done", title: "Received 2 files", detail: "from Pixel", peer: "Pixel", via: "quickshare", bytes: 18_000_000, paths: ["/tmp/a.jpg"]),
            ActivityItem(id: "2", direction: "out", state: "working", title: "Sending 1 file", detail: "to Office Mac", peer: "Office Mac", via: "whoosh", bytes: 12_000_000, total: 48_000_000, paths: []),
        ]
        return model
    }

    private static func emptyModel() -> AppModel {
        let model = base()
        model.receiving = false
        model.sawPeers = true
        return model
    }

    private static func offerModel() -> AppModel {
        let model = populated()
        model.currentOffer = Offer(
            id: "offer",
            via: "quickshare",
            peer: "Pixel",
            pin: "4821",
            files: [
                OfferFile(name: "vacation.jpg", bytes: 4_200_000, mime: "image/jpeg", kind: "photo"),
                OfferFile(name: "notes.pdf", bytes: 88_000, mime: "application/pdf", kind: "file"),
            ]
        )
        return model
    }

    private static func trustModel() -> AppModel {
        let model = populated()
        model.peers[0].trusted = false
        model.trustPeer = model.peers[0]
        return model
    }

    private static func stoppedModel() -> AppModel {
        let model = emptyModel()
        model.engineDown = true
        model.engineError = "The Whoosh engine is missing from the app."
        return model
    }
}

private func attachPreviews(_ model: AppModel) {
    let scale = NSScreen.main?.backingScaleFactor ?? 2
    var images: [String: NSImage] = [:]
    for path in model.files.map(\.path) where FileManager.default.fileExists(atPath: path) {
        if let image = MediaThumbnail.make(path: path, scale: scale) {
            images[path] = MediaThumbnail.thumbnail(image)
        }
    }
    if !images.isEmpty {
        model.thumbnails = images
    }
}

extension ActivityItem {
    init(id: String, direction: String, state: String, title: String, detail: String, peer: String, via: String, bytes: UInt64?, total: UInt64? = nil, paths: [String]) {
        self.id = id
        self.direction = direction
        self.state = state
        self.title = title
        self.detail = detail
        self.peer = peer
        self.via = via
        self.bytes = bytes
        self.total = total
        self.paths = paths
    }
}
