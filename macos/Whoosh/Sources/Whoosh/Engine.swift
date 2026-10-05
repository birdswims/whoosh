import Darwin
import Foundation

/// Talks to the bundled `whoosh app` process. One JSON command per line in,
/// one JSON event per line out.
final class Engine {
    var onEvent: ((WireEvent) -> Void)?
    var onExit: ((Int32, String) -> Void)?

    private var process: Process?
    private var input: FileHandle?
    private var pipes: [Pipe] = []
    private let lock = NSLock()
    private var stderrText = ""
    private var intentionalStop = false

    func start() {
        if process?.isRunning == true {
            return
        }
        guard let binary = Self.locateBinary() else {
            DispatchQueue.main.async { [weak self] in
                self?.onExit?(1, "The Whoosh engine is missing from the app.")
            }
            return
        }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: binary)
        process.arguments = ["app"]
        let stdin = Pipe()
        let stdout = Pipe()
        let stderr = Pipe()
        process.standardInput = stdin
        process.standardOutput = stdout
        process.standardError = stderr
        process.terminationHandler = { [weak self] process in
            DispatchQueue.main.async {
                guard let self else { return }
                guard self.process === process else { return }
                self.process = nil
                self.input = nil
                if !self.intentionalStop {
                    self.onExit?(process.terminationStatus, self.stderrTail())
                }
            }
        }
        do {
            try process.run()
        } catch {
            DispatchQueue.main.async { [weak self] in
                self?.onExit?(1, error.localizedDescription)
            }
            return
        }
        self.process = process
        input = stdin.fileHandleForWriting
        pipes = [stdin, stdout, stderr]
        intentionalStop = false
        let outHandle = stdout.fileHandleForReading
        let errHandle = stderr.fileHandleForReading
        Thread.detachNewThread { [weak self] in
            self?.readLines(outHandle)
        }
        Thread.detachNewThread { [weak self] in
            self?.readStderr(errHandle)
        }
    }

    func send(_ command: Command) {
        guard let input else { return }
        let encoder = JSONEncoder()
        encoder.keyEncodingStrategy = .convertToSnakeCase
        guard var data = try? encoder.encode(command) else { return }
        data.append(0x0A)
        do {
            try input.write(contentsOf: data)
        } catch {
            return
        }
    }

    /// Asks the engine to exit, then kills it and every process it started.
    ///
    /// SIGTERM does not stop the engine: its runtime installs a handler and
    /// keeps the signal. `dns-sd` also calls `setsid`, so it outlives the
    /// engine unless this walks the child list and kills those pids too.
    func stopAndWait() {
        intentionalStop = true
        send(Command(id: "bye", op: "shutdown"))
        try? input?.close()
        input = nil
        guard let running = process else { return }
        if waitUntilStopped(running, timeout: 2.5) {
            running.waitUntilExit()
            return
        }
        forceStop(running)
        _ = waitUntilStopped(running, timeout: 1)
        if !running.isRunning {
            running.waitUntilExit()
        }
    }

    private func waitUntilStopped(_ process: Process, timeout: TimeInterval) -> Bool {
        let deadline = Date().addingTimeInterval(timeout)
        while process.isRunning, Date() < deadline {
            Thread.sleep(forTimeInterval: 0.05)
        }
        return !process.isRunning
    }

    private func forceStop(_ process: Process) {
        let pid = process.processIdentifier
        guard pid > 1 else { return }
        for _ in 0..<8 {
            let children = descendantPids(of: pid)
            if children.isEmpty {
                break
            }
            for child in children {
                kill(child, SIGKILL)
            }
            Thread.sleep(forTimeInterval: 0.02)
        }
        if process.isRunning {
            kill(pid, SIGKILL)
        }
    }

    private func stderrTail() -> String {
        lock.lock()
        defer { lock.unlock() }
        return stderrText
    }

    private func readLines(_ handle: FileHandle) {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        let fd = handle.fileDescriptor
        var buffer = Data()
        var storage = [UInt8](repeating: 0, count: 8192)
        while true {
            let count = storage.withUnsafeMutableBytes { raw -> Int in
                guard let base = raw.baseAddress else { return 0 }
                return Darwin.read(fd, base, raw.count)
            }
            if count < 0 {
                if errno == EINTR { continue }
                break
            }
            if count == 0 { break }
            buffer.append(contentsOf: storage.prefix(count))
            while let newline = buffer.firstIndex(of: 0x0A) {
                let line = buffer.subdata(in: buffer.startIndex..<newline)
                buffer.removeSubrange(buffer.startIndex...newline)
                guard !line.isEmpty else { continue }
                do {
                    let event = try decoder.decode(WireEvent.self, from: line)
                    DispatchQueue.main.async { [weak self] in
                        self?.onEvent?(event)
                    }
                } catch {
                    let text = String(data: line, encoding: .utf8) ?? ""
                    fputs("whoosh: ignored event \(error) \(text.prefix(180))\n", stderr)
                }
            }
        }
    }

    private func readStderr(_ handle: FileHandle) {
        let fd = handle.fileDescriptor
        var storage = [UInt8](repeating: 0, count: 4096)
        while true {
            let count = storage.withUnsafeMutableBytes { raw -> Int in
                guard let base = raw.baseAddress else { return 0 }
                return Darwin.read(fd, base, raw.count)
            }
            if count < 0 {
                if errno == EINTR { continue }
                break
            }
            if count == 0 { break }
            guard let text = String(bytes: storage.prefix(count), encoding: .utf8), !text.isEmpty else {
                continue
            }
            lock.lock()
            stderrText.append(text)
            if stderrText.count > 12_000 {
                stderrText.removeFirst(stderrText.count - 8_000)
            }
            lock.unlock()
        }
    }

    static func locateBinary() -> String? {
        let fileManager = FileManager.default
        var candidates: [String] = []
        if let override = ProcessInfo.processInfo.environment["WHOOSH_BIN"] {
            candidates.append(override)
        }
        if let executable = Bundle.main.executableURL?.deletingLastPathComponent() {
            candidates.append(executable.appendingPathComponent("whoosh-core").path)
            candidates.append(executable.appendingPathComponent("whoosh").path)
        }
        let argv = URL(fileURLWithPath: CommandLine.arguments[0]).deletingLastPathComponent()
        candidates.append(argv.appendingPathComponent("whoosh-core").path)
        candidates.append(argv.appendingPathComponent("whoosh").path)
        let relative = [
            "../../../../target/release/whoosh",
            "../../../../target/debug/whoosh",
            "../../../../../target/release/whoosh",
            "../../../../../target/debug/whoosh",
            "target/release/whoosh",
            "target/debug/whoosh",
        ]
        for suffix in relative {
            candidates.append(argv.appendingPathComponent(suffix).path)
            candidates.append(URL(fileURLWithPath: fileManager.currentDirectoryPath).appendingPathComponent(suffix).path)
        }
        for path in candidates {
            if fileManager.isExecutableFile(atPath: path) {
                return path
            }
        }
        return nil
    }
}

@_silgen_name("proc_listchildpids")
private func proc_listchildpids(
    _ ppid: Int32,
    _ buffer: UnsafeMutableRawPointer?,
    _ bufsize: Int32
) -> Int32

/// `proc_listchildpids` writes pids at the front of the buffer. Its return is
/// either that count or a byte count; extra slots stay zero and are dropped.
private func childPids(_ pid: pid_t) -> [pid_t] {
    var buffer = [pid_t](repeating: 0, count: 256)
    let wrote = buffer.withUnsafeMutableBytes { raw in
        proc_listchildpids(pid, raw.baseAddress, Int32(raw.count))
    }
    guard wrote > 0 else { return [] }
    let count = min(buffer.count, Int(wrote))
    return Array(buffer.prefix(count)).filter { $0 > 1 }
}

private func descendantPids(of root: pid_t) -> [pid_t] {
    var ordered: [pid_t] = []
    var seen: Set<pid_t> = [root]
    func walk(_ pid: pid_t, _ depth: Int) {
        if depth > 8 { return }
        for child in childPids(pid) where seen.insert(child).inserted {
            walk(child, depth + 1)
            ordered.append(child)
        }
    }
    walk(root, 0)
    return ordered
}
