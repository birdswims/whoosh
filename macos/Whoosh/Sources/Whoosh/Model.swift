import AppKit
import Observation
import UniformTypeIdentifiers
import WhooshAirDrop

enum AppScreen {
    case home
    case settings
    case activity
}

@Observable
final class AppModel {
    static weak var shared: AppModel?
    private static var pendingURLs: [URL] = []

    private let engine = Engine()
    private let airDropSender = AirDropSender()
    private var wired = false
    private var didShutdown = false
    private var appliedPrefs = false
    private var nextIdentifier = 0
    private var respondedOffers: Set<String> = []
    private var offerQueue: [Offer] = []
    private var bannerToken = 0
    private var thumbnailLoads: Set<String> = []
    private var pasteboardTimer: Timer?
    private var pasteboardChange = NSPasteboard.general.changeCount
    private var suppressPasteboardChange: Int?
    private let maxClipboardText = 1024 * 1024
    private let maxClipboardImage = 8 * 1024 * 1024
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
    var screen = AppScreen.home
    var peers: [Peer] = []
    var primaryPeers: [Peer] { PeerGrouping.primary(in: peers) }
    var hiddenPeers: [Peer] { PeerGrouping.hidden(in: peers) }
    var hiddenDevicesTitle: String { "Hidden devices (\(hiddenPeers.count))" }
    var trustedDevices: [TrustedDevice] = []
    private var trustedReady = false
    var selectedPeerID: String?
    var files: [SendFile] = []
    var thumbnails: [String: NSImage] = [:]
    var outgoingPin = ""
    var activity: [ActivityItem] = []

    var liveOutgoing: ActivityItem? {
        activity.first { $0.direction == "out" && $0.state == "working" }
    }

    var incomingNow: [ActivityItem] {
        Array(activity.filter { $0.direction == "in" && $0.state == "working" }.prefix(3))
    }
    var currentOffer: Offer?
    var trustPeer: Peer?
    var trustForClipboard = false
    var banner: String?
    var engineDown = false
    var engineError = ""
    var sending = false

    var selectedPeer: Peer? {
        peers.first { $0.id == selectedPeerID }
    }

    var canSend: Bool {
        selectedPeer != nil && !files.isEmpty && !engineDown && currentOffer == nil && trustPeer == nil && !sending
    }

    var sendTitle: String {
        if sending {
            return "Sending…"
        }
        if let name = selectedPeer?.name, !name.isEmpty {
            return "Send to \(name)"
        }
        return "Send"
    }

    init() {
        Self.shared = self
        selectedPeerID = UserDefaults.standard.string(forKey: Pref.selected)
        if !Self.pendingURLs.isEmpty {
            let queued = Self.pendingURLs
            Self.pendingURLs.removeAll()
            addURLs(queued)
        }
    }

    /// Files opened while the window model does not exist yet, including a share.
    static func accept(_ urls: [URL]) {
        let files = urls.flatMap(shareFiles)
        guard !files.isEmpty else { return }
        if let shared {
            shared.addURLs(files)
            return
        }
        pendingURLs.append(contentsOf: files)
    }

    /// whoosh://add?manifest= points at a file list written by the share extension.
    private static func shareFiles(_ url: URL) -> [URL] {
        guard url.scheme?.lowercased() == "whoosh" else { return [url] }
        guard let components = URLComponents(url: url, resolvingAgainstBaseURL: false),
              let manifest = components.queryItems?.first(where: { $0.name == "manifest" })?.value,
              let text = try? String(contentsOfFile: manifest, encoding: .utf8) else {
            return []
        }
        return text.split(whereSeparator: \.isNewline).compactMap { line in
            let path = String(line)
            guard !path.isEmpty, FileManager.default.fileExists(atPath: path) else { return nil }
            return URL(fileURLWithPath: path)
        }
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
        // Reopening the window calls this again. A running engine already has
        // the saved preferences, and resetting that flag would send them twice.
        guard !engine.isRunning else {
            watchPasteboard()
            return
        }
        didShutdown = false
        engineDown = false
        appliedPrefs = false
        engine.start()
        watchPasteboard()
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
        pasteboardTimer?.invalidate()
        pasteboardTimer = nil
        airDropSender.cancel()
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
        guard !sending, let peer = selectedPeer, !files.isEmpty else { return }
        if peer.via == "whoosh", !peer.trusted {
            trustForClipboard = false
            trustPeer = peer
            return
        }
        beginSend(peer, trust: peer.via == "whoosh")
    }

    func copyFrom(_ peer: Peer) {
        guard peer.via == "whoosh", !engineDown else { return }
        guard let fingerprint = peer.fingerprint, !fingerprint.isEmpty else {
            showBanner("That device did not share a fingerprint.")
            return
        }
        if !peer.trusted {
            trustForClipboard = true
            trustPeer = peer
            return
        }
        pullClipboard(peer, trust: false)
    }

    func confirmTrust() {
        guard let peer = trustPeer else { return }
        let clipboard = trustForClipboard
        trustForClipboard = false
        trustPeer = nil
        if clipboard {
            pullClipboard(peer, trust: true)
            return
        }
        beginSend(peer, trust: true)
    }

    func cancelTrust() {
        trustForClipboard = false
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

    /// Accept or decline one queued request, including one answered from a notification.
    func decide(_ id: String, accept: Bool) {
        respond(id, accept: accept)
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

    func refreshTrusted() {
        engine.send(Command(id: nextID(), op: "trusted"))
    }

    func showSettings() {
        if screen == .settings {
            screen = .home
            return
        }
        screen = .settings
        refreshTrusted()
    }

    func showActivity() {
        screen = screen == .activity ? .home : .activity
    }

    func showHome() {
        screen = .home
    }

    func addNearby(_ peer: Peer) {
        guard peer.via == "whoosh", !engineDown, !peer.trusted else { return }
        guard let fingerprint = peer.fingerprint, !fingerprint.isEmpty else {
            showBanner("That device did not share a fingerprint.")
            return
        }
        if fingerprint.caseInsensitiveCompare(self.fingerprint) == .orderedSame {
            showBanner("That fingerprint is this computer.")
            return
        }
        let name = peer.name.trimmingCharacters(in: .whitespacesAndNewlines)
        engine.send(Command(
            id: nextID(),
            op: "trust",
            fingerprint: fingerprint,
            peerName: name.isEmpty ? nil : String(name.prefix(64))
        ))
    }

    func removeTrusted(_ fingerprint: String) {
        guard !engineDown, !fingerprint.isEmpty else { return }
        engine.send(Command(id: nextID(), op: "untrust", fingerprint: fingerprint))
    }

    func trustedTitle(for device: TrustedDevice) -> String {
        let saved = device.name.trimmingCharacters(in: .whitespacesAndNewlines)
        if !saved.isEmpty {
            return saved
        }
        if let peer = peers.first(where: { peer in
            peer.via == "whoosh"
                && (peer.fingerprint ?? "").caseInsensitiveCompare(device.fingerprint) == .orderedSame
                && !peer.name.isEmpty
        }) {
            return peer.name
        }
        return device.isSelf ? "This computer" : "Trusted device"
    }

    private func beginSend(_ peer: Peer, trust: Bool) {
        sending = true
        if peer.via == "airdrop" {
            sendAirDrop(peer)
            return
        }
        performSend(peer, trust: trust)
    }

    private func sendAirDrop(_ peer: Peer) {
        let id = UUID().uuidString
        let count = files.count
        let title = count == 1 ? "Sending 1 file" : "Sending \(count) files"
        func event(_ state: String, _ title: String, _ detail: String, bytes: UInt64? = nil, total: UInt64? = nil) -> WireEvent {
            WireEvent(ev: "activity", id: id, via: "airdrop", peer: peer.name,
                      direction: "out", state: state, title: title, detail: detail, bytes: bytes, total: total)
        }
        guard let target = peer.airdropTarget else {
            record(event("failed", "Could not send", "The device's AirDrop details are unavailable. Refresh nearby devices and try again."))
            return
        }
        record(event("working", title, "preparing files for \(peer.name)"))
        airDropSender.send(to: target, name: peer.name, files: files.map { URL(fileURLWithPath: $0.path) }) { [weak self] update in
            guard let self else { return }
            switch update {
            case .preparing:
                self.record(event("working", title, "preparing files for \(peer.name)"))
            case .requestingAcceptance:
                self.record(event("working", title, "requesting acceptance on \(peer.name)"))
            case .transferring(let bytes, let total):
                self.record(event("working", title, "transferring to \(peer.name)", bytes: bytes, total: total))
            case .completed(let bytes):
                self.record(event("done", count == 1 ? "Sent 1 file" : "Sent \(count) files", "to \(peer.name)", bytes: bytes, total: bytes))
            case .failed(let message):
                self.record(event("failed", "Could not send", message))
            case .cancelled:
                self.record(event("failed", "AirDrop cancelled", "The AirDrop transfer was cancelled."))
            }
        }
    }

    private func watchPasteboard() {
        if pasteboardTimer == nil {
            pasteboardChange = NSPasteboard.general.changeCount
            let timer = Timer(timeInterval: 0.4, repeats: true) { [weak self] _ in
                self?.pollPasteboard()
            }
            RunLoop.main.add(timer, forMode: .common)
            pasteboardTimer = timer
        }
        publishPasteboard()
    }

    private func pollPasteboard() {
        let count = NSPasteboard.general.changeCount
        guard count != pasteboardChange else { return }
        pasteboardChange = count
        if count == suppressPasteboardChange { return }
        publishPasteboard()
    }

    private func publishPasteboard() {
        let board = NSPasteboard.general
        var text = board.string(forType: .string) ?? ""
        let textTooBig = text.utf8.count > maxClipboardText
        if textTooBig {
            text = ""
        }
        var png = board.data(forType: .png)
        if png == nil, let tiff = board.data(forType: .tiff), let rep = NSBitmapImageRep(data: tiff) {
            png = rep.representation(using: .png, properties: [:])
        }
        var imageTooBig = false
        if let data = png, data.count > maxClipboardImage {
            png = nil
            imageTooBig = true
        }
        if text.isEmpty, png == nil, textTooBig || imageTooBig {
            return
        }
        engine.send(Command(
            id: nextID(),
            op: "clipboard",
            text: text,
            imagePng: png?.base64EncodedString()
        ))
    }

    private func pullClipboard(_ peer: Peer, trust: Bool) {
        engine.send(Command(
            id: nextID(),
            op: "clipboard-pull",
            target: peer.address,
            trust: trust,
            fingerprint: peer.fingerprint,
            peerName: peer.name
        ))
        showBanner("Copying from \(peer.name)…")
    }

    private func applyRemoteClipboard(_ event: WireEvent) {
        if event.ok == false {
            let message = event.error ?? ""
            showBanner(message.isEmpty ? "Couldn't copy that clipboard." : message)
            return
        }
        let text = event.text ?? ""
        let png = event.imagePng.flatMap { encoded -> Data? in
            encoded.isEmpty ? nil : Data(base64Encoded: encoded)
        }
        if text.isEmpty, png == nil || png?.isEmpty == true {
            let name = event.peer?.isEmpty == false ? event.peer! : "That device"
            showBanner("\(name) has nothing on the clipboard.")
            return
        }
        let board = NSPasteboard.general
        var types: [NSPasteboard.PasteboardType] = []
        if !text.isEmpty { types.append(.string) }
        if let png, !png.isEmpty { types.append(.png) }
        board.declareTypes(types, owner: nil)
        if !text.isEmpty {
            board.setString(text, forType: .string)
        }
        if let png, !png.isEmpty {
            board.setData(png, forType: .png)
        }
        let count = board.changeCount
        suppressPasteboardChange = count
        pasteboardChange = count
        let name = event.peer?.isEmpty == false ? event.peer! : "that device"
        if !text.isEmpty, png?.isEmpty == false {
            showBanner("Copied text and an image from \(name).")
        } else if png?.isEmpty == false {
            showBanner("Copied an image from \(name).")
        } else {
            showBanner("Copied from \(name).")
        }
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
        OfferNotifications.shared.clear(id)
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
                sending = false
                showBanner(error)
            }
        case "clipboard":
            applyRemoteClipboard(event)
        case "trusted":
            applyTrusted(event.devices ?? [])
        case "peers":
            sawPeers = true
            let incoming = event.peers ?? []
            if let selected = selectedPeerID, !incoming.contains(where: { $0.id == selected }) {
                let previous = peers.first { $0.id == selected }
                if let match = incoming.first(where: { peer in
                    if "\(peer.via)|\(peer.address)" == selected {
                        return true
                    }
                    guard let previous, previous.via == peer.via else { return false }
                    if previous.name == peer.name {
                        return true
                    }
                    // An iPhone keeps one row, but its service name changes
                    // between advertisements. Stay on that row.
                    return previous.via == "airdrop" && incoming.filter { $0.via == "airdrop" }.count == 1 && peer.via == "airdrop"
                }) {
                    selectedPeerID = match.id
                    UserDefaults.standard.set(match.id, forKey: Pref.selected)
                }
            }
            peers = incoming
            if trustedReady {
                syncPeerTrust()
            }
            if selectedPeerID == nil {
                selectedPeerID = primaryPeers.first?.id
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
            Session.announceOffer(offer)
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

    private func applyTrusted(_ incoming: [TrustedDevice]) {
        trustedReady = true
        trustedDevices = incoming
        syncPeerTrust()
    }

    private func syncPeerTrust() {
        let allowed = Set(trustedDevices.map { $0.fingerprint.lowercased() })
        for index in peers.indices where peers[index].via == "whoosh" {
            let fingerprint = (peers[index].fingerprint ?? "").lowercased()
            let trusted = !fingerprint.isEmpty && allowed.contains(fingerprint)
            if peers[index].trusted != trusted {
                peers[index].trusted = trusted
            }
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
        if event.direction == "out", event.state == "done" || event.state == "failed" {
            sending = false
        }
        if event.direction == "out", event.state == "done" {
            let name = event.peer ?? ""
            let via = event.via ?? ""
            for index in peers.indices where peers[index].name == name && peers[index].via == via {
                peers[index].trusted = true
            }
            if banner != nil {
                banner = nil
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
        sending = airDropSender.isSending
        receiving = false
        let line = message.split(whereSeparator: \.isNewline).last.map(String.init)?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        engineError = line.isEmpty ? "Whoosh stopped unexpectedly." : String(line.prefix(280))
        Session.announceNotice(engineError)
    }

    private func showBanner(_ text: String) {
        banner = text
        bannerToken += 1
        let token = bannerToken
        DispatchQueue.main.asyncAfter(deadline: .now() + 6) { [weak self] in
            guard let self, self.bannerToken == token else { return }
            self.banner = nil
        }
        Session.announceNotice(text)
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
