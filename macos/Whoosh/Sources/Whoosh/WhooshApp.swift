import AppKit
import SwiftUI

@main
struct WhooshApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel()

    var body: some Scene {
        Window("Whoosh", id: "main") {
            RootView()
                .environment(model)
                .frame(minWidth: 900, minHeight: 640)
                .onAppear { model.start() }
        }
        .windowStyle(.hiddenTitleBar)
        .defaultSize(width: 1000, height: 720)
        .commands {
            CommandGroup(replacing: .newItem) {
                Button("Add Files…") { model.chooseFiles() }
                    .keyboardShortcut("o")
                Button("Send") { model.send() }
                    .keyboardShortcut(.return, modifiers: .command)
                    .disabled(!model.canSend)
                Divider()
                Button(model.receiving ? "Stop Receiving" : "Start Receiving") {
                    model.setReceiving(!model.receiving)
                }
                .keyboardShortcut("r")
            }
        }
        .commands {
            WhooshWindowCommands()
        }
    }
}

private struct WhooshWindowCommands: Commands {
    var body: some Commands {
        CommandGroup(replacing: .appSettings) {
            Button("Settings…") {
                AppModel.shared?.showSettings()
            }
            .keyboardShortcut(",", modifiers: .command)
        }
        CommandGroup(after: .windowList) {
            Button("Activity") {
                AppModel.shared?.showActivity()
            }
            .keyboardShortcut("a", modifiers: [.command, .shift])
        }
    }
}

final class AppDelegate: NSObject, NSApplicationDelegate {
    func applicationDidFinishLaunching(_ notification: Notification) {
        if let style = ProcessInfo.processInfo.environment["WHOOSH_APPEARANCE"] {
            switch style {
            case "dark":
                NSApp.appearance = NSAppearance(named: .darkAqua)
            case "light":
                NSApp.appearance = NSAppearance(named: .aqua)
            default:
                break
            }
        }
        NSApp.activate()
        MainActor.assumeIsolated {
            Snapshot.writeIfRequested()
        }
        DispatchQueue.global(qos: .utility).async {
            registerShareExtension()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        true
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        AppModel.shared?.shutdown()
        return .terminateNow
    }

    func applicationWillTerminate(_ notification: Notification) {
        AppModel.shared?.shutdown()
    }

    func application(_ application: NSApplication, open urls: [URL]) {
        DispatchQueue.main.async {
            AppModel.accept(urls)
        }
    }
}

/// Puts Whoosh in the system Share menu. New extensions are registered but
/// not offered until they are enabled.
private func registerShareExtension() {
    guard let plugins = Bundle.main.builtInPlugInsURL else { return }
    let appex = plugins.appendingPathComponent("WhooshShare.appex")
    guard FileManager.default.fileExists(atPath: appex.path) else { return }
    let pluginkit = URL(fileURLWithPath: "/usr/bin/pluginkit")
    run(pluginkit, ["-a", appex.path])
    run(pluginkit, ["-e", "use", "-i", "com.whoosh.macos.share"])
}

private func run(_ executable: URL, _ arguments: [String]) {
    let process = Process()
    process.executableURL = executable
    process.arguments = arguments
    process.standardOutput = FileHandle.nullDevice
    process.standardError = FileHandle.nullDevice
    try? process.run()
    process.waitUntilExit()
}
