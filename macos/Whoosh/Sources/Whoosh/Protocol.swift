import Foundation
import WhooshAirDrop
import WhooshUI

struct Command: Encodable {
    var id: String
    var op: String
    var name: String?
    var dir: String?
    var native: Bool?
    var quickshare: Bool?
    var airdrop: Bool?
    var sortMedia: Bool?
    var requirePin: Bool?
    var enabled: Bool?
    var via: String?
    var target: String?
    var files: [String]?
    var pin: String?
    var trust: Bool?
    var fingerprint: String?
    var accept: Bool?
    var offer: String?
    var peerName: String?
    var text: String? = nil
    var imagePng: String? = nil
}

struct Peer: Identifiable, Decodable, Hashable {
    var id: String
    var via: String
    var name: String
    var detail: String
    var address: String
    var fingerprint: String?
    var trusted: Bool
    var trustsUs: Bool = false
    var airdropTarget: AirDropTarget?
}

struct OfferFile: Decodable, Hashable {
    var sizeKnown: Bool? = nil
    var isSizeKnown: Bool { sizeKnown ?? (bytes > 0) }
    var name: String
    var bytes: UInt64
    var mime: String
    var kind: String
}

struct WireEvent: Decodable {
    var ev: String
    var id: String?
    var ok: Bool?
    var error: String?
    var version: String?
    var device: String?
    var fingerprint: String?
    var receiving: Bool?
    var name: String?
    var dir: String?
    var pin: String?
    var native: Bool?
    var quickshare: Bool?
    var airdrop: Bool?
    var sortMedia: Bool?
    var requirePin: Bool?
    var visibility: String?
    var address: String?
    var nativePort: Int?
    var warnings: [String]?
    var peers: [Peer]?
    var devices: [TrustedDevice]?
    var via: String?
    var peer: String?
    var files: [OfferFile]?
    var accepted: Bool?
    var direction: String?
    var state: String?
    var title: String?
    var detail: String?
    var bytes: UInt64?
    var total: UInt64? = nil
    var paths: [String]?
    var text: String? = nil
    var imagePng: String? = nil
}

struct TrustedDevice: Decodable, Identifiable, Hashable {
    var fingerprint: String
    var name: String
    var addresses: [String] = []
    var isSelf: Bool = false
    var id: String { fingerprint }
}

extension Peer {
    var showsClipboard: Bool {
        via == "whoosh" && trusted && trustsUs && !(fingerprint ?? "").isEmpty
    }

    var showsWaiting: Bool {
        via == "whoosh" && trusted && !trustsUs && !(fingerprint ?? "").isEmpty
    }

    var showsAdd: Bool {
        via == "whoosh" && !trusted && !(fingerprint ?? "").isEmpty
    }

    var addAccessLabel: String { "Add \(name)" }

    var waitingText: String { "Waiting for \(name) to add this computer" }

    var clipboardTip: String { "Copy the clipboard from this device" }

    var clipboardAccessLabel: String { "Copy clipboard from \(name)" }
}

struct Offer: Identifiable, Equatable {
    var id: String
    var via: String
    var peer: String
    var pin: String?
    var files: [OfferFile]

    var totalBytes: UInt64 {
        files.reduce(0) { $0 + $1.bytes }
    }

    var sizeUnknown: Bool {
        files.contains { !$0.isSizeKnown }
    }
}

struct SendFile: Identifiable, Hashable {
    var path: String
    var name: String
    var id: String { path }
}

struct ActivityItem: Identifiable, Equatable {
    var id: String
    var direction: String
    var state: String
    var title: String
    var detail: String
    var peer: String
    var via: String
    var bytes: UInt64?
    var total: UInt64?
    var paths: [String]

    var percent: Double {
        guard let total, total > 0 else { return 0 }
        return min(1, Double(bytes ?? 0) / Double(total))
    }

    var showsDeterminate: Bool {
        state == "working" && (total ?? 0) > 0
    }

    var showsIndeterminate: Bool {
        state == "working" && !showsDeterminate && (bytes ?? 0) > 0
    }

    var showsProgress: Bool {
        showsDeterminate || showsIndeterminate
    }

    var progressLabel: String {
        guard let total, total > 0 else { return "" }
        let transferred = min(bytes ?? 0, total)
        let pct = UInt64((Double(transferred) / Double(total) * 100).rounded(.down))
        return "\(min(pct, 100))%"
    }

    var subtitle: String {
        if state == "done", let bytes {
            return "\(detail) · \(humanSize(bytes))"
        }
        if state == "working", let total, total > 0 {
            let transferred = min(bytes ?? 0, total)
            let pct = UInt64((Double(transferred) / Double(total) * 100).rounded(.down))
            return "\(detail) · \(humanSize(transferred)) of \(humanSize(total)) · \(min(pct, 100))%"
        }
        if state == "working", let bytes, bytes > 0 {
            return "\(detail) · \(humanSize(bytes))"
        }
        return detail
    }

    init(_ event: WireEvent) {
        id = event.id ?? UUID().uuidString
        direction = event.direction ?? ""
        state = event.state ?? ""
        title = event.title ?? ""
        detail = event.detail ?? ""
        peer = event.peer ?? ""
        via = event.via ?? ""
        bytes = event.bytes
        total = event.total
        paths = event.paths ?? []
    }

    mutating func apply(_ event: WireEvent, keepID: Bool) {
        if !keepID, let id = event.id {
            self.id = id
        }
        if let value = event.direction { direction = value }
        if let value = event.state { state = value }
        if let value = event.title { title = value }
        if let value = event.detail { detail = value }
        if let value = event.peer { peer = value }
        if let value = event.via { via = value }
        if let value = event.bytes { bytes = value }
        if let value = event.total { total = value }
        if let value = event.paths, !value.isEmpty { paths = value }
    }
}

enum Pref {
    static let name = "whoosh.name"
    static let dir = "whoosh.dir"
    static let native = "whoosh.native"
    static let quickshare = "whoosh.quickshare"
    static let airdrop = "whoosh.airdrop"
    static let sortMedia = "whoosh.sortMedia"
    static let requirePin = "whoosh.requirePin"
    static let receiving = "whoosh.receiving"
    static let selected = "whoosh.selectedPeer"
}

func protocolLabel(_ via: String) -> String {
    switch via {
    case "whoosh": "Whoosh"
    case "quickshare": "Quick Share"
    case "airdrop": "AirDrop"
    default: "Device"
    }
}

/// One nearby row per computer. Whoosh wins when that computer runs it.
/// Quick Share is next, then AirDrop. Matches `NearbyDevices` on Windows.
enum PeerGrouping {
    static func primary(in peers: [Peer]) -> [Peer] {
        split(peers).primary
    }

    static func hidden(in peers: [Peer]) -> [Peer] {
        split(peers).hidden
    }

    static func split(_ peers: [Peer]) -> (primary: [Peer], hidden: [Peer]) {
        var parent = Array(peers.indices)
        func find(_ index: Int) -> Int {
            var index = index
            while parent[index] != index {
                parent[index] = parent[parent[index]]
                index = parent[index]
            }
            return index
        }
        func union(_ left: Int, _ right: Int) {
            let a = find(left)
            let b = find(right)
            if a != b {
                parent[b] = a
            }
        }

        var hosts: [String: Int] = [:]
        var names: [String: Int] = [:]
        for (index, peer) in peers.enumerated() {
            if let host = hostKey(peer.address) {
                if let other = hosts[host] {
                    union(index, other)
                } else {
                    hosts[host] = index
                }
            }
            let trimmed = peer.name.trimmingCharacters(in: .whitespacesAndNewlines)
            guard distinctive(trimmed) else { continue }
            let key = trimmed.lowercased()
            if let named = names[key] {
                if peers[named].via.caseInsensitiveCompare(peer.via) != .orderedSame {
                    union(index, named)
                }
            } else {
                names[key] = index
            }
        }

        var groups: [Int: [Int]] = [:]
        for index in peers.indices {
            groups[find(index), default: []].append(index)
        }
        var hiddenIndexes = Set<Int>()
        for indexes in groups.values where indexes.count > 1 {
            let best = indexes.min { prefer(peers[$0], peers[$1]) }!
            for index in indexes where index != best {
                hiddenIndexes.insert(index)
            }
        }
        var primary: [Peer] = []
        var hidden: [Peer] = []
        for (index, peer) in peers.enumerated() {
            if hiddenIndexes.contains(index) {
                hidden.append(peer)
            } else {
                primary.append(peer)
            }
        }
        return (primary, hidden)
    }

    private static func prefer(_ candidate: Peer, _ current: Peer) -> Bool {
        let left = (rank(candidate.via), candidate.name.lowercased(), candidate.id)
        let right = (rank(current.via), current.name.lowercased(), current.id)
        return left < right
    }

    private static func rank(_ via: String) -> Int {
        switch via {
        case "whoosh": 0
        case "quickshare": 1
        case "airdrop": 2
        default: 3
        }
    }

    private static func distinctive(_ name: String) -> Bool {
        if name.isEmpty { return false }
        let generic = ["quick share device", "airdrop device", "device"]
        return !generic.contains(name.lowercased())
    }

    private static func hostKey(_ address: String) -> String? {
        let trimmed = address.trimmingCharacters(in: .whitespacesAndNewlines)
        if trimmed.hasPrefix("[") {
            guard let end = trimmed.firstIndex(of: "]") else { return nil }
            let inside = trimmed[trimmed.index(after: trimmed.startIndex)..<end]
            let text = inside.lowercased()
            if text.hasPrefix("::ffff:"), text.split(separator: ":").count > 2 {
                return String(text.dropFirst(7))
            }
            return text.isEmpty ? nil : text
        }
        guard let colon = trimmed.lastIndex(of: ":"), colon > trimmed.startIndex else { return nil }
        let host = trimmed[..<colon]
        if host.isEmpty || host.contains(":") { return nil }
        return String(host).lowercased()
    }
}
