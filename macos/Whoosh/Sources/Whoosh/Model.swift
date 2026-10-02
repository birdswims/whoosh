import AppKit
import Observation
import UniformTypeIdentifiers

@Observable
final class AppModel {
    static weak var shared: AppModel?

    private let engine = Engine()
    private var wired = false
    private var didShutdown = false
    private var appliedPrefs = false
    private var nextIdentifier = 0
    private var respondedOffers: Set<String> = []
    private var offerQueue: [Offer] = []
    private var bannerToken = 0
    private var thumbnailLoads: Set<String> = []
    private let thumbnailQueue: OperationQueue = {
        let queue = OperationQueue()
        queue.name = "whoosh.thumbnails"
        queue.maxConcurrentOperationCount = 4
        queue.qualityOfService = .utility
        return queue
    }()

    var version = ""
    var receiving = false
    var deviceName = ""
    var saveDir = ""
    var fingerprint = ""
    var pin: String?
    var native = true
    var quickshare = true
    var airdrop = true
    var sortMedia = false
    var requirePin = false
    var visibility = ""
    var address: String?
    var warnings: [String] = []
    var sawPeers = false
    var peers: [Peer] = []
    var selectedPeerID: String?
    var files: [SendFile] = []
    var thumbnails: [String: NSImage] = [:]
    var outgoingPin = ""
    var activity: [ActivityItem] = []
    var currentOffer: Offer?
    var trustPeer: Peer?
    var banner: String?
    var engineDown = false
    var engineError = ""

    var selectedPeer: Peer? {
        peers.first { $0.id == selectedPeerID }
    }

    var canSend: Bool {
        selectedPeer != nil && !files.isEmpty && !engineDown && currentOffer == nil && trustPeer == nil
    }

    var sendTitle: String {
        if let name = selectedPeer?.name, !name.isEmpty {
            return "Send to \(name)"
        }
        return "Send"
    }

    init() {
        Self.shared = self
        selectedPeerID = UserDefaults.standard.string(forKey: Pref.selected)
    }

    func start() {
        if !wired {
            wired = true
            engine.onEvent = { [weak self] event in
                self?.handle(event)
            }
            engine.onExit = { [weak self] _, message in
                self?.handleExit(message)
            }
        }
        didShutdown = false
        engineDown = false
        appliedPrefs = false
        engine.start()
    }

    func retry() {
        engineDown = false
        engineError = ""
        appliedPrefs = false
        engine.start()
    }

    func shutdown() {
        guard !didShutdown else { return }
        didShutdown = true
        engine.stopAndWait()
    }

    func setReceiving(_ enabled: Bool) {
        receiving = enabled
        UserDefaults.standard.set(enabled, forKey: Pref.receiving)
        engine.send(Command(id: nextID(), op: "receive", enabled: enabled))
    }

    func pushConfig() {
        engine.send(Command(
            id: nextID(),
            op: "configure",
            name: deviceName.isEmpty ? nil : deviceName,
            dir: saveDir.isEmpty ? nil : saveDir,
            native: native,
            quickshare: quickshare,
            airdrop: airdrop,
            sortMedia: sortMedia,
            requirePin: requirePin
        ))
    }

    func applyName(_ raw: String) {
        let trimmed = raw.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty, trimmed != deviceName else { return }
        deviceName = String(trimmed.prefix(64))
        UserDefaults.standard.set(deviceName, forKey: Pref.name)
        pushConfig()
    }

    func chooseSaveFolder() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = false
        panel.canChooseDirectories = true
        panel.canCreateDirectories = true
        panel.allowsMultipleSelection = false
        panel.prompt = "Choose"
        panel.message = "Received files are saved in this folder."
        if !saveDir.isEmpty {
            panel.directoryURL = URL(fileURLWithPath: saveDir)
        }
        guard panel.runModal() == .OK, let url = panel.url else { return }
        saveDir = url.path
        UserDefaults.standard.set(saveDir, forKey: Pref.dir)
        pushConfig()
    }

    func revealSaveFolder() {
        guard !saveDir.isEmpty else { return }
        let url = URL(fileURLWithPath: saveDir)
        try? FileManager.default.createDirectory(at: url, withIntermediateDirectories: true)
        NSWorkspace.shared.open(url)
    }

    func chooseFiles() {
        let panel = NSOpenPanel()
        panel.canChooseFiles = true
        panel.canChooseDirectories = true
        panel.allowsMultipleSelection = true
        panel.prompt = "Add"
        panel.message = "Files and folders to send."
        guard panel.runModal() == .OK else { return }
        addURLs(panel.urls)
    }

    func takeDrop(_ providers: [NSItemProvider]) -> Bool {
        let type = UTType.fileURL.identifier
        let supported = providers.filter { $0.hasItemConformingToTypeIdentifier(type) }
        guard !supported.isEmpty else { return false }
        for provider in supported {
            provider.loadItem(forTypeIdentifier: type, options: nil) { item, _ in
                guard let url = Self.url(from: item) else { return }
                DispatchQueue.main.async { [weak self] in
                    self?.addURLs([url])
                }
            }
        }
        return true
    }

    func addURLs(_ urls: [URL]) {
        var seen = Set(files.map(\.path))
        var next = files
        var added: [String] = []
        var truncated = false
        for url in urls {
            for file in expand(url) {
                if next.count >= 500 {
                    truncated = true
                    break
                }
                let path = file.standardizedFileURL.path
                if seen.insert(path).inserted {
                    next.append(SendFile(path: path, name: file.lastPathComponent))
                    added.append(path)
                }
            }
        }
        files = next
        loadThumbnails(added)
        if truncated {
            showBanner("Whoosh sends up to 500 files at a time.")
        }
    }

    func removeFile(_ file: SendFile) {
        files.removeAll { $0.path == file.path }
        thumbnails.removeValue(forKey: file.path)
    }

    func select(_ peer: Peer) {
        selectedPeerID = peer.id
        UserDefaults.standard.set(peer.id, forKey: Pref.selected)
    }

    func send() {
        guard let peer = selectedPeer, !files.isEmpty else { return }
        if peer.via == "whoosh", !peer.trusted {
            trustPeer = peer
            return
        }
        performSend(peer, trust: peer.via == "whoosh")
    }

    func confirmTrust() {
        guard let peer = trustPeer else { return }
        trustPeer = nil
        performSend(peer, trust: true)
    }

    func cancelTrust() {
        trustPeer = nil
    }

    func acceptCurrentOffer() {
        guard let offer = currentOffer else { return }
        respond(offer.id, accept: true)
    }

    func declineCurrentOffer() {
        guard let offer = currentOffer else { return }
        respond(offer.id, accept: false)
    }

    func showActivity(_ item: ActivityItem) {
        let urls = item.paths.map { URL(fileURLWithPath: $0) }
        guard !urls.isEmpty else { return }
        NSWorkspace.shared.activateFileViewerSelecting(urls)
    }

    var canClearActivity: Bool {
        activity.contains { $0.state != "working" }
    }

    func clearActivity() {
        activity.removeAll { $0.state != "working" }
    }

    func copyFingerprint() {
        guard !fingerprint.isEmpty else { return }
        NSPasteboard.general.clearContents()
        NSPasteboard.general.setString(fingerprint, forType: .string)
        showBanner("Fingerprint copied.")
    }

    private func performSend(_ peer: Peer, trust: Bool) {
        let pin = outgoingPin.trimmingCharacters(in: .whitespacesAndNewlines)
        engine.send(Command(
            id: nextID(),
            op: "send",
            via: peer.via,
            target: peer.address,
            files: files.map(\.path),
            pin: peer.via == "whoosh" && !pin.isEmpty ? pin : nil,
            trust: peer.via == "whoosh" ? trust : nil,
            fingerprint: peer.via == "whoosh" ? peer.fingerprint : nil,
            peerName: peer.name
        ))
    }

    private func respond(_ id: String, accept: Bool) {
        if respondedOffers.insert(id).inserted {
            engine.send(Command(id: nextID(), op: "decide", accept: accept, offer: id))
        }
        finishOffer(id)
    }

    private func finishOffer(_ id: String) {
        offerQueue.removeAll { $0.id == id }
        guard currentOffer?.id == id else { return }
        currentOffer = offerQueue.isEmpty ? nil : offerQueue.removeFirst()
    }

    private func handle(_ event: WireEvent) {
        switch event.ev {
        case "hello":
            if let version = event.version { self.version = version }
            if let device = event.device, deviceName.isEmpty { deviceName = device }
            if let fingerprint = event.fingerprint { self.fingerprint = fingerprint }
            applySavedPreferencesIfNeeded()
        case "status":
            applyStatus(event)
            applySavedPreferencesIfNeeded()
        case "ack":
            if event.ok == false, let error = event.error, !error.isEmpty {
                showBanner(error)
            }
        case "peers":
            sawPeers = true
            let incoming = event.peers ?? []
            if let selected = selectedPeerID, !incoming.contains(where: { $0.id == selected }) {
                let previous = peers.first { $0.id == selected }
                if let match = incoming.first(where: { peer in
                    "\(peer.via)|\(peer.address)" == selected
                        || previous.map { $0.via == peer.via && $0.name == peer.name } == true
                }) {
                    selectedPeerID = match.id
                    UserDefaults.standard.set(match.id, forKey: Pref.selected)
                }
            }
            peers = incoming
            if selectedPeerID == nil {
                selectedPeerID = peers.first?.id
            }
        case "offer":
            guard let id = event.id else { return }
            let offer = Offer(
                id: id,
                via: event.via ?? "",
                peer: event.peer ?? "Someone",
                pin: event.pin,
                files: event.files ?? []
            )
            if currentOffer == nil {
                currentOffer = offer
            } else {
                offerQueue.append(offer)
            }
        case "offer_resolved":
            if let id = event.id {
                respondedOffers.insert(id)
                finishOffer(id)
            }
        case "activity":
            record(event)
        default:
            break
        }
    }

    private func applyStatus(_ event: WireEvent) {
        if let value = event.receiving { receiving = value }
        if let value = event.name { deviceName = value }
        if let value = event.dir { saveDir = value }
        if let value = event.fingerprint { fingerprint = value }
        pin = event.pin
        if let value = event.native { native = value }
        if let value = event.quickshare { quickshare = value }
        if let value = event.airdrop { airdrop = value }
        if let value = event.sortMedia { sortMedia = value }
        if let value = event.requirePin { requirePin = value }
        if let value = event.visibility { visibility = value }
        address = event.address
        if let value = event.warnings { warnings = value }
    }

    private func applySavedPreferencesIfNeeded() {
        guard !appliedPrefs else { return }
        appliedPrefs = true
        let defaults = UserDefaults.standard
        var command = Command(id: nextID(), op: "configure")
        var any = false
        if let name = defaults.string(forKey: Pref.name), !name.isEmpty {
            command.name = name
            any = true
        }
        if let dir = defaults.string(forKey: Pref.dir), !dir.isEmpty {
            command.dir = dir
            any = true
        }
        if defaults.object(forKey: Pref.native) != nil {
            command.native = defaults.bool(forKey: Pref.native)
            any = true
        }
        if defaults.object(forKey: Pref.quickshare) != nil {
            command.quickshare = defaults.bool(forKey: Pref.quickshare)
            any = true
        }
        if defaults.object(forKey: Pref.airdrop) != nil {
            command.airdrop = defaults.bool(forKey: Pref.airdrop)
            any = true
        }
        if defaults.object(forKey: Pref.sortMedia) != nil {
            command.sortMedia = defaults.bool(forKey: Pref.sortMedia)
            any = true
        }
        if defaults.object(forKey: Pref.requirePin) != nil {
            command.requirePin = defaults.bool(forKey: Pref.requirePin)
            any = true
        }
        if any {
            engine.send(command)
        }
        let enabled = defaults.object(forKey: Pref.receiving) as? Bool ?? true
        engine.send(Command(id: nextID(), op: "receive", enabled: enabled))
    }

    private func record(_ event: WireEvent) {
        let incoming = ActivityItem(event)
        if let index = activity.firstIndex(where: { $0.id == incoming.id }) {
            activity[index].apply(event, keepID: true)
        } else if event.direction == "in", event.state == "done" || event.state == "failed" {
            let peer = event.peer ?? ""
            if let index = activity.firstIndex(where: { row in
                row.direction == "in" && row.state == "working" && (row.peer == peer || row.peer.isEmpty || peer.isEmpty)
            }) {
                activity[index].apply(event, keepID: true)
            } else {
                activity.insert(incoming, at: 0)
            }
        } else {
            activity.insert(incoming, at: 0)
        }
        if activity.count > 40 {
            activity.removeLast(activity.count - 40)
        }
        if event.direction == "out", event.state == "done" {
            let name = event.peer ?? ""
            let via = event.via ?? ""
            for index in peers.indices where peers[index].name == name && peers[index].via == via {
                peers[index].trusted = true
            }
        }
        if event.state == "failed" {
            let message = event.detail?.isEmpty == false ? event.detail! : (event.title ?? "Transfer failed")
            showBanner(message)
        }
    }

    private func handleExit(_ message: String) {
        guard !didShutdown else { return }
        engineDown = true
        receiving = false
        let line = message.split(whereSeparator: \.isNewline).last.map(String.init)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        engineError = line.isEmpty ? "Whoosh stopped unexpectedly." : String(line.prefix(280))
    }

    private func showBanner(_ text: String) {
        banner = text
        bannerToken += 1
        let token = bannerToken
        DispatchQueue.main.asyncAfter(deadline: .now() + 6) { [weak self] in
            guard let self, self.bannerToken == token else { return }
            self.banner = nil
        }
    }

    private func nextID() -> String {
        nextIdentifier += 1
        return String(nextIdentifier)
    }

    private func loadThumbnails(_ paths: [String]) {
        let pending = paths.filter { path in
            guard MediaThumbnail.kind(of: path) != nil else { return false }
            guard thumbnails[path] == nil else { return false }
            return thumbnailLoads.insert(path).inserted
        }
        guard !pending.isEmpty else { return }
        let scale = NSScreen.main?.backingScaleFactor ?? 2
        for path in pending {
            thumbnailQueue.addOperation {
                let image = MediaThumbnail.make(path: path, scale: scale)
                DispatchQueue.main.async {
                    AppModel.shared?.storeThumbnail(path, image)
                }
            }
        }
    }

    private func storeThumbnail(_ path: String, _ image: CGImage?) {
        thumbnailLoads.remove(path)
        guard let image, files.contains(where: { $0.path == path }) else { return }
        thumbnails[path] = MediaThumbnail.thumbnail(image)
    }

    private func expand(_ url: URL) -> [URL] {
        let keys: Set<URLResourceKey> = [.isDirectoryKey, .isPackageKey, .isRegularFileKey, .isHiddenKey]
        guard let values = try? url.resourceValues(forKeys: keys) else { return [] }
        if values.isHidden == true || url.lastPathComponent.hasPrefix(".") {
            return []
        }
        if values.isPackage == true {
            return []
        }
        if values.isDirectory == true {
            guard let enumerator = FileManager.default.enumerator(
                at: url,
                includingPropertiesForKeys: [.isRegularFileKey],
                options: [.skipsHiddenFiles, .skipsPackageDescendants]
            ) else { return [] }
            var found: [URL] = []
            for case let item as URL in enumerator {
                let fileValues = try? item.resourceValues(forKeys: [.isRegularFileKey])
                if fileValues?.isRegularFile == true {
                    found.append(item)
                }
                if found.count >= 500 {
                    break
                }
            }
            return found
        }
        if values.isRegularFile == true {
            return [url]
        }
        return []
    }

    private static func url(from item: Any?) -> URL? {
        if let url = item as? URL {
            return url
        }
        if let data = item as? Data {
            return URL(dataRepresentation: data, relativeTo: nil)
        }
        return nil
    }
}
