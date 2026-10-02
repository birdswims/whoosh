import Foundation
import WhooshAirDrop

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
}

struct Peer: Identifiable, Decodable, Hashable {
    var id: String
    var via: String
    var name: String
    var detail: String
    var address: String
    var fingerprint: String?
    var trusted: Bool
    var airdropTarget: AirDropTarget?
}

struct OfferFile: Decodable, Hashable {
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
    var via: String?
    var peer: String?
    var files: [OfferFile]?
    var accepted: Bool?
    var direction: String?
    var state: String?
    var title: String?
    var detail: String?
    var bytes: UInt64?
    var paths: [String]?
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
        !files.isEmpty && files.allSatisfy { $0.bytes == 0 }
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
    var paths: [String]

    init(_ event: WireEvent) {
        id = event.id ?? UUID().uuidString
        direction = event.direction ?? ""
        state = event.state ?? ""
        title = event.title ?? ""
        detail = event.detail ?? ""
        peer = event.peer ?? ""
        via = event.via ?? ""
        bytes = event.bytes
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
