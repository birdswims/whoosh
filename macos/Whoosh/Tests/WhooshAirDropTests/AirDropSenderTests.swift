import XCTest
@testable import WhooshAirDrop

final class AirDropSenderTests: XCTestCase {
    /// Opt-in physical-device check; never sends to a nearby device implicitly.
    func testLiveDirectTransfer() throws {
        guard let json = ProcessInfo.processInfo.environment["WHOOSH_AIRDROP_LIVE_TARGET"] else {
            throw XCTSkip("Set WHOOSH_AIRDROP_LIVE_TARGET to the explicitly selected device metadata")
        }
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let target = try decoder.decode(AirDropTarget.self, from: Data(json.utf8))
        let file = FileManager.default.temporaryDirectory.appendingPathComponent("whoosh-direct-test.txt")
        try Data("hello from whoosh direct sender\n".utf8).write(to: file)
        defer { try? FileManager.default.removeItem(at: file) }
        let sender = AirDropSender()
        let finished = expectation(description: "system confirmed delivery")
        var terminal: AirDropUpdate?
        sender.send(to: target, name: "iPad", files: [file]) { update in
            print("AirDrop integration: \(update)")
            switch update {
            case .completed, .failed, .cancelled:
                terminal = update
                finished.fulfill()
            default: break
            }
        }
        wait(for: [finished], timeout: 100)
        XCTAssertFalse(sender.isSending)
        guard case .completed(bytes: 32) = terminal else { return XCTFail("Transfer did not complete: \(String(describing: terminal))") }
        sender.cancel() // Completion must have detached the callback/context.
    }

    func testAcceptanceAndFullByteProgressAreNotCompletion() {
        let result: NSDictionary = ["BytesCopied": 18, "TotalBytes": 18]
        for event: Int64 in [5, 6, 7] {
            guard case .update(.transferring(bytes: 18, total: 18)) = SenderEvent.decode(event, result) else {
                return XCTFail("Only a completion callback confirms delivery")
            }
        }
        guard case .update(.completed(bytes: 18)) = SenderEvent.decode(9, result) else {
            return XCTFail("System completion was lost")
        }
    }

    func testPrepareRequiresResumeAndFailurePreservesSystemError() {
        guard case .resume = SenderEvent.decode(2, [:]) else { return XCTFail() }
        let error = NSError(domain: "SFOperation", code: -1, userInfo: [NSLocalizedDescriptionKey: "The recipient declined."])
        guard case .update(.failed("The recipient declined.")) = SenderEvent.decode(10, ["Error": error]) else { return XCTFail() }
        guard case .update(.cancelled) = SenderEvent.decode(4, [:]) else { return XCTFail() }
        guard case .ignore = SenderEvent.decode(999, [:]) else { return XCTFail() }
    }

    func testTargetKeepsExactServiceIdentityFromEngine() throws {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let target = try decoder.decode(AirDropTarget.self, from: Data(#"{"service_name":"2fbeedeb424c","host_name":"ipad.local","port":8770,"flags":111611}"#.utf8))
        XCTAssertTrue(target.isValid)
        XCTAssertEqual(target.serviceName, "2fbeedeb424c")
        XCTAssertEqual(target.flags, 111611)
        XCTAssertFalse(AirDropTarget(serviceName: "", hostName: "ipad.local", port: 8770, flags: nil).isValid)
        XCTAssertFalse(AirDropTarget(serviceName: "id", hostName: "", port: 8770, flags: nil).isValid)
        XCTAssertFalse(AirDropTarget(serviceName: "id", hostName: "ipad.local", port: 0, flags: nil).isValid)
    }

    func testUnavailableFileFailsBeforeStartingSystemTransfer() {
        let sender = AirDropSender()
        var updates: [AirDropUpdate] = []
        sender.send(to: AirDropTarget(serviceName: "id", hostName: "ipad.local", port: 8770, flags: nil),
                    name: "iPad", files: [URL(fileURLWithPath: "/nonexistent/\(UUID().uuidString)")]) { updates.append($0) }
        XCTAssertFalse(sender.isSending)
        XCTAssertEqual(updates.count, 1)
        guard case .failed = updates[0] else { return XCTFail() }
        sender.cancel()
        XCTAssertEqual(updates.count, 1)
    }
}
