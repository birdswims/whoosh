import Foundation
import CoreFoundation
import Darwin

public struct AirDropTarget: Decodable, Hashable {
    public let serviceName: String
    public let hostName: String
    public let port: UInt32
    public let flags: UInt64?

    public init(serviceName: String, hostName: String, port: UInt32, flags: UInt64?) {
        self.serviceName = serviceName
        self.hostName = hostName
        self.port = port
        self.flags = flags
    }

    var isValid: Bool {
        !serviceName.isEmpty && !hostName.isEmpty && (1...65535).contains(port)
    }
}

public enum AirDropUpdate: Equatable {
    case preparing
    case requestingAcceptance
    case transferring(bytes: UInt64, total: UInt64?)
    case completed(bytes: UInt64)
    case failed(String)
    case cancelled
}

/// SFOperation's sender event contract. A request or byte-progress callback
/// is not delivery confirmation: only event 9 marks the transfer completed.
enum SenderEvent {
    case resume
    case update(AirDropUpdate)
    case ignore

    static func decode(_ event: Int64, _ result: NSDictionary) -> SenderEvent {
        let bytes = (result["BytesCopied"] as? NSNumber)?.uint64Value ?? 0
        let total = (result["TotalBytes"] as? NSNumber)?.uint64Value
        switch event {
        case 2: return .resume
        case 3: return .update(.requestingAcceptance)
        case 4: return .update(.cancelled)
        case 5, 6, 7: return .update(.transferring(bytes: bytes, total: total))
        case 9: return .update(.completed(bytes: bytes))
        case 10:
            let message = (result["Error"] as? NSError)?.localizedDescription
                ?? "macOS could not complete the AirDrop transfer. Keep the device nearby and try again."
            return .update(.failed(message))
        case 11: return .update(.preparing)
        default: return .ignore
        }
    }
}

/// Direct AirDrop via macOS's transfer service. Discovery remains in the
/// Whoosh engine; the selected Bonjour instance is passed verbatim. This
/// invokes neither a recipient picker nor the restricted system browser.
public final class AirDropSender {
    private var api: SharingAPI?
    private var operation: CFTypeRef?
    private var callback: ((AirDropUpdate) -> Void)?
    private var timer: Timer?
    private var transferring = false
    public var isSending: Bool { operation != nil }

    public init() {}

    public func send(to target: AirDropTarget, name: String, files: [URL], update: @escaping (AirDropUpdate) -> Void) {
        precondition(Thread.isMainThread)
        guard !isSending else {
            update(.failed("An AirDrop transfer is already in progress."))
            return
        }
        guard target.isValid, !files.isEmpty, files.allSatisfy({ $0.isFileURL && FileManager.default.isReadableFile(atPath: $0.path) }) else {
            update(.failed("The device or selected files are no longer available. Refresh the device and try again."))
            return
        }
        do {
            let api = try self.api ?? SharingAPI()
            self.api = api
            guard let node = api.createNode(nil, name as CFString, target.serviceName as CFString)?.takeRetainedValue(),
                  let operation = api.createOperation(nil, api.kindSender)?.takeRetainedValue() else {
                throw SenderError.unavailable
            }
            api.setServiceName(node, target.serviceName as CFString)
            api.setHostName(node, target.hostName as CFString)
            api.setDomain(node, "local." as CFString)
            api.setDisplayName(node, name as CFString)
            api.setComputerName(node, name as CFString)
            api.setKinds(node, NSSet(array: api.nodeKinds))
            api.setProtocols(node, [api.protocolAirDrop] as CFArray)
            api.setPort(node, target.port)
            if let flags = target.flags { api.setFlags(node, NSNumber(value: flags)) }
            api.setProperty(operation, api.nodeKey, node)
            api.setProperty(operation, api.itemsKey, files as CFArray)
            api.setProperty(operation, api.bundleKey, (Bundle.main.bundleIdentifier ?? "com.whoosh.macos") as CFString)
            api.setProperty(operation, api.sessionKey, UUID().uuidString as CFString)

            self.operation = operation
            self.callback = update
            transferring = false
            // SFOperation retains the callback context. The context holds a weak
            // owner, so late callbacks cannot keep a sender alive or use freed memory.
            let box = CallbackBox(owner: self)
            var context = CFStreamClientContext(
                version: 0, info: Unmanaged.passUnretained(box).toOpaque(),
                retain: { pointer in
                    guard let pointer else { return nil }
                    return Unmanaged<CallbackBox>.fromOpaque(pointer).retain().toOpaque()
                },
                release: { pointer in
                    guard let pointer else { return }
                    Unmanaged<CallbackBox>.fromOpaque(pointer).release()
                }, copyDescription: nil)
            api.setClient(operation, { operation, event, result, info in
                guard let info else { return }
                let box = Unmanaged<CallbackBox>.fromOpaque(info).takeUnretainedValue()
                box.owner?.receive(operation, event, result.map { $0 as NSDictionary } ?? [:])
            }, &context)
            api.setQueue(operation, DispatchQueue.main)
            armTimeout()
            update(.preparing)
            api.resume(operation)
        } catch {
            update(.failed(error.localizedDescription))
        }
    }

    public func cancel() {
        precondition(Thread.isMainThread)
        guard isSending else { return }
        finish(.cancelled)
    }

    private func receive(_ source: CFTypeRef, _ event: Int64, _ result: NSDictionary) {
        precondition(Thread.isMainThread)
        guard let operation, source === operation else { return }
        switch SenderEvent.decode(event, result) {
        case .resume: api?.resume(operation)
        case .ignore: break
        case .update(let update):
            switch update {
            case .completed, .failed, .cancelled: finish(update)
            case .transferring:
                transferring = true
                armTimeout()
                callback?(update)
            case .preparing, .requestingAcceptance: callback?(update)
            }
        }
    }

    private func armTimeout() {
        timer?.invalidate()
        timer = Timer.scheduledTimer(withTimeInterval: transferring ? 120 : 90, repeats: false) { [weak self] _ in
            guard let self else { return }
            let message = self.transferring
                ? "AirDrop stopped responding during the transfer. Delivery could not be confirmed."
                : "The device did not answer the AirDrop request. Keep it unlocked and nearby, then try again."
            self.finish(.failed(message))
        }
    }

    private func finish(_ update: AirDropUpdate) {
        let callback = self.callback
        self.callback = nil
        teardown()
        callback?(update)
    }

    private func teardown() {
        timer?.invalidate()
        timer = nil
        guard let operation, let api else { return }
        self.operation = nil
        var context = CFStreamClientContext(version: 0, info: nil, retain: nil, release: nil, copyDescription: nil)
        api.setClient(operation, nil, &context)
        api.cancel(operation)
    }

    deinit { teardown() }
}

private final class CallbackBox {
    weak var owner: AirDropSender?
    init(owner: AirDropSender) { self.owner = owner }
}

private enum SenderError: LocalizedError {
    case unavailable
    var errorDescription: String? { "Direct AirDrop is unavailable on this macOS version." }
}

/// Private framework symbols are resolved at runtime so an unsupported OS
/// produces an ordinary transfer error instead of preventing app launch.
private final class SharingAPI {
    typealias Create = @convention(c) (CFAllocator?, CFString) -> Unmanaged<CFTypeRef>?
    typealias CreateNode = @convention(c) (CFAllocator?, CFString, CFString) -> Unmanaged<CFTypeRef>?
    typealias SetNode = @convention(c) (CFTypeRef, CFTypeRef) -> Void
    typealias SetPort = @convention(c) (CFTypeRef, UInt32) -> Void
    typealias SetProperty = @convention(c) (CFTypeRef, CFString, CFTypeRef) -> Void
    typealias Callback = @convention(c) (CFTypeRef, Int64, CFDictionary?, UnsafeMutableRawPointer?) -> Void
    typealias SetClient = @convention(c) (CFTypeRef, Callback?, UnsafeMutablePointer<CFStreamClientContext>) -> Void
    typealias SetQueue = @convention(c) (CFTypeRef, DispatchQueue) -> Void
    typealias Action = @convention(c) (CFTypeRef) -> Void

    let createNode: CreateNode
    let createOperation: Create
    let setServiceName, setHostName, setDomain, setDisplayName, setComputerName, setKinds, setProtocols, setFlags: SetNode
    let setPort: SetPort
    let setProperty: SetProperty
    let setClient: SetClient
    let setQueue: SetQueue
    let resume, cancel: Action
    let kindSender, nodeKey, itemsKey, bundleKey, sessionKey, protocolAirDrop: CFString
    let nodeKinds: [CFString]

    init() throws {
        guard let handle = dlopen("/System/Library/PrivateFrameworks/Sharing.framework/Sharing", RTLD_LAZY | RTLD_LOCAL) else {
            throw SenderError.unavailable
        }
        // Keep the framework loaded for process lifetime: callbacks may be queued.
        func symbol<T>(_ name: String, _: T.Type) throws -> T {
            guard let pointer = dlsym(handle, name) else { throw SenderError.unavailable }
            return unsafeBitCast(pointer, to: T.self)
        }
        func constant(_ name: String) throws -> CFString {
            guard let pointer = dlsym(handle, name) else { throw SenderError.unavailable }
            return pointer.assumingMemoryBound(to: CFString.self).pointee
        }
        createNode = try symbol("SFNodeCreate", CreateNode.self)
        createOperation = try symbol("SFOperationCreate", Create.self)
        setServiceName = try symbol("SFNodeSetServiceName", SetNode.self)
        setHostName = try symbol("SFNodeSetHostName", SetNode.self)
        setDomain = try symbol("SFNodeSetDomain", SetNode.self)
        setDisplayName = try symbol("SFNodeSetDisplayName", SetNode.self)
        setComputerName = try symbol("SFNodeSetComputerName", SetNode.self)
        setKinds = try symbol("SFNodeSetKinds", SetNode.self)
        setProtocols = try symbol("SFNodeSetBonjourProtocols", SetNode.self)
        setFlags = try symbol("SFNodeSetFlags", SetNode.self)
        setPort = try symbol("SFNodeSetPortNumber", SetPort.self)
        setProperty = try symbol("SFOperationSetProperty", SetProperty.self)
        setClient = try symbol("SFOperationSetClient", SetClient.self)
        setQueue = try symbol("SFOperationSetDispatchQueue", SetQueue.self)
        resume = try symbol("SFOperationResume", Action.self)
        cancel = try symbol("SFOperationCancel", Action.self)
        kindSender = try constant("kSFOperationKindSender")
        nodeKey = try constant("kSFOperationNodeKey")
        itemsKey = try constant("kSFOperationItemsKey")
        bundleKey = try constant("kSFOperationBundleIDKey")
        sessionKey = try constant("kSFOperationSessionIDKey")
        protocolAirDrop = try constant("kSFNodeProtocolAirDrop")
        nodeKinds = try [constant("kSFNodeKindAirDrop"), constant("kSFNodeKindBonjour"), constant("kSFNodeKindPerson")]
    }
}
