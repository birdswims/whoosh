import AppKit
import UniformTypeIdentifiers

/// Share-menu entry. The send list and nearby devices live in the main app,
/// so this hands the shared items to that app and closes the share sheet.
@objc(WhooshShareViewController)
public final class ShareViewController: NSViewController {
    private let status = NSTextField(labelWithString: "Adding to Whoosh…")
    private var started = false
    private var finished = false

    public override func loadView() {
        let root = NSView(frame: NSRect(x: 0, y: 0, width: 280, height: 64))
        status.translatesAutoresizingMaskIntoConstraints = false
        status.font = .systemFont(ofSize: 13)
        status.lineBreakMode = .byTruncatingTail
        root.addSubview(status)
        NSLayoutConstraint.activate([
            status.leadingAnchor.constraint(equalTo: root.leadingAnchor, constant: 16),
            status.trailingAnchor.constraint(equalTo: root.trailingAnchor, constant: -16),
            status.centerYAnchor.constraint(equalTo: root.centerYAnchor),
            root.widthAnchor.constraint(greaterThanOrEqualToConstant: 260),
            root.heightAnchor.constraint(equalToConstant: 64),
        ])
        view = root
    }

    public override func viewDidAppear() {
        super.viewDidAppear()
        guard !started else { return }
        started = true
        let providers = (extensionContext?.inputItems as? [NSExtensionItem] ?? [])
            .flatMap { $0.attachments ?? [] }
        guard !providers.isEmpty else {
            finish(nil, message: "Nothing to add.")
            return
        }
        let group = DispatchGroup()
        var urls: [URL] = []
        for provider in providers {
            group.enter()
            materialize(provider) { url in
                if let url {
                    urls.append(url)
                }
                group.leave()
            }
        }
        group.notify(queue: .main) { [weak self] in
            self?.finish(urls.isEmpty ? nil : urls, message: "Whoosh could not read that item.")
        }
        DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak self] in
            guard let self, !self.finished else { return }
            self.finish(urls.isEmpty ? nil : urls, message: "Whoosh could not read that item.")
        }
    }

    private func materialize(_ provider: NSItemProvider, done: @escaping (URL?) -> Void) {
        let types = [
            UTType.fileURL.identifier,
            UTType.url.identifier,
            UTType.image.identifier,
            UTType.movie.identifier,
            UTType.plainText.identifier,
            UTType.data.identifier,
        ].filter { provider.hasItemConformingToTypeIdentifier($0) }
        attempt(provider, types: types, index: 0, done: done)
    }

    private func attempt(_ provider: NSItemProvider, types: [String], index: Int, done: @escaping (URL?) -> Void) {
        guard index < types.count else {
            DispatchQueue.main.async { done(nil) }
            return
        }
        let type = types[index]
        let suggested = provider.suggestedName
        provider.loadItem(forTypeIdentifier: type, options: nil) { [weak self] item, _ in
            if let url = Self.store(item, type: type, suggested: suggested) {
                DispatchQueue.main.async { done(url) }
                return
            }
            guard let self else {
                DispatchQueue.main.async { done(nil) }
                return
            }
            self.attempt(provider, types: types, index: index + 1, done: done)
        }
    }

    private func finish(_ urls: [URL]?, message: String) {
        guard !finished else { return }
        guard let urls, !urls.isEmpty else {
            finished = true
            fail(message)
            return
        }
        finished = true
        guard let app = Self.containingApp() else {
            fail("Whoosh is not available.")
            return
        }
        let configuration = NSWorkspace.OpenConfiguration()
        configuration.activates = true
        configuration.promptsUserIfNeeded = false
        NSWorkspace.shared.open(urls, withApplicationAt: app, configuration: configuration) { [weak self] _, error in
            DispatchQueue.main.async {
                guard let self else { return }
                if error != nil {
                    self.openWithLink(urls)
                    return
                }
                self.extensionContext?.completeRequest(returningItems: nil)
            }
        }
    }

    /// Sandboxed share extensions cannot always hand file URLs to another app.
    /// The list is written where the unsandboxed Whoosh app can read it.
    private func openWithLink(_ urls: [URL]) {
        let readable = urls.compactMap { Self.readableCopy($0) }
        guard let manifest = Self.writeManifest(readable), let link = Self.whooshLink(manifest) else {
            fail("Whoosh could not read that item.")
            return
        }
        extensionContext?.open(link) { [weak self] success in
            DispatchQueue.main.async {
                guard let self else { return }
                if success {
                    self.extensionContext?.completeRequest(returningItems: nil)
                    return
                }
                self.fail("Whoosh is not available.")
            }
        }
    }

    private func fail(_ message: String) {
        status.stringValue = message
        extensionContext?.cancelRequest(withError: NSError(
            domain: "Whoosh",
            code: 1,
            userInfo: [NSLocalizedDescriptionKey: message]
        ))
    }

    private static func containingApp() -> URL? {
        var url = Bundle.main.bundleURL
        for _ in 0..<8 {
            if url.pathExtension == "app" {
                return url
            }
            let parent = url.deletingLastPathComponent()
            if parent.path == url.path {
                break
            }
            url = parent
        }
        return nil
    }

    private static func store(_ item: Any?, type: String, suggested: String?) -> URL? {
        if let url = item as? URL {
            return url.isFileURL ? durable(url) : write(Data(url.absoluteString.utf8), name: safeName(suggested, fallback: "link.txt"))
        }
        if let url = item as? NSURL {
            return store(url as URL, type: type, suggested: suggested)
        }
        if let text = item as? String {
            let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
            if trimmed.hasPrefix("file:"), let url = URL(string: trimmed), url.isFileURL {
                return durable(url)
            }
            if trimmed.hasPrefix("/"), FileManager.default.fileExists(atPath: trimmed) {
                return durable(URL(fileURLWithPath: trimmed))
            }
            if type == UTType.url.identifier || trimmed.contains("://") {
                return write(Data(trimmed.utf8), name: safeName(suggested, fallback: "link.txt"))
            }
            guard !trimmed.isEmpty else { return nil }
            return write(Data(trimmed.utf8), name: safeName(suggested, fallback: "shared.txt"))
        }
        if let image = item as? NSImage {
            guard let tiff = image.tiffRepresentation,
                  let rep = NSBitmapImageRep(data: tiff),
                  let png = rep.representation(using: .png, properties: [:]) else {
                return nil
            }
            return write(png, name: safeName(suggested, fallback: "image.png"))
        }
        if let data = item as? Data, !data.isEmpty {
            if let text = String(data: data, encoding: .utf8) {
                let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
                if trimmed.hasPrefix("file:"), let url = URL(string: trimmed), url.isFileURL {
                    return durable(url)
                }
                if trimmed.hasPrefix("/"), FileManager.default.fileExists(atPath: trimmed) {
                    return durable(URL(fileURLWithPath: trimmed))
                }
            }
            let fallback = type == UTType.image.identifier ? "image.png" : "shared.bin"
            return write(data, name: safeName(suggested, fallback: fallback))
        }
        return nil
    }

    /// Finder shares a real path. A share sheet can also hand over a temporary
    /// file that disappears when this extension exits, so those are copied.
    private static func durable(_ url: URL) -> URL? {
        let scoped = url.startAccessingSecurityScopedResource()
        defer {
            if scoped {
                url.stopAccessingSecurityScopedResource()
            }
        }
        let path = url.path
        let temporary = path.contains("/TemporaryItems/")
            || path.contains("/tmp/")
            || path.contains("/Containers/")
            || path.hasPrefix(NSTemporaryDirectory())
        if !temporary, FileManager.default.fileExists(atPath: path) {
            return url
        }
        return copyIntoInbox(url)
    }

    private static func readableCopy(_ url: URL) -> URL? {
        let scoped = url.startAccessingSecurityScopedResource()
        defer {
            if scoped {
                url.stopAccessingSecurityScopedResource()
            }
        }
        var directory: ObjCBool = false
        if FileManager.default.fileExists(atPath: url.path, isDirectory: &directory), directory.boolValue {
            return url
        }
        return copyIntoInbox(url)
    }

    private static func writeManifest(_ urls: [URL]) -> URL? {
        let lines = urls.map(\.path).filter { !$0.isEmpty && !$0.contains("\n") && !$0.contains("\r") }
        guard !lines.isEmpty else { return nil }
        let text = lines.joined(separator: "\n") + "\n"
        return write(Data(text.utf8), name: "manifest.txt")
    }

    private static func whooshLink(_ manifest: URL) -> URL? {
        var components = URLComponents()
        components.scheme = "whoosh"
        components.host = "add"
        components.queryItems = [URLQueryItem(name: "manifest", value: manifest.path)]
        return components.url
    }

    private static func copyIntoInbox(_ url: URL) -> URL? {
        let inbox = FileManager.default.temporaryDirectory.appendingPathComponent("whoosh-share", isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: inbox, withIntermediateDirectories: true)
            let name = safeName(url.lastPathComponent, fallback: "shared")
            let dest = inbox.appendingPathComponent(UUID().uuidString + "-" + name)
            try FileManager.default.copyItem(at: url, to: dest)
            return dest
        } catch {
            return FileManager.default.fileExists(atPath: url.path) ? url : nil
        }
    }

    private static func write(_ data: Data, name: String) -> URL? {
        let inbox = FileManager.default.temporaryDirectory.appendingPathComponent("whoosh-share", isDirectory: true)
        do {
            try FileManager.default.createDirectory(at: inbox, withIntermediateDirectories: true)
            let dest = inbox.appendingPathComponent(UUID().uuidString + "-" + name)
            try data.write(to: dest, options: .atomic)
            return dest
        } catch {
            return nil
        }
    }

    private static func safeName(_ suggested: String?, fallback: String) -> String {
        let raw = suggested?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        let cleaned = raw
            .replacingOccurrences(of: "/", with: "-")
            .replacingOccurrences(of: ":", with: "-")
        return cleaned.isEmpty ? fallback : cleaned
    }
}

@used
public func retainShareController() {
    _ = ShareViewController.self
}
