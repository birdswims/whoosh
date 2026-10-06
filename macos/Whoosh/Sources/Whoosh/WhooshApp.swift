import AppKit
import SwiftUI

@main
struct WhooshApp: App {
    @NSApplicationDelegateAdaptor(AppDelegate.self) private var delegate
    @State private var model = AppModel()
    @AppStorage(Pref.runInBackground) private var runInBackground = false
    /// MenuBarExtra writes this binding on every update. AppStorage would
    /// post a defaults change and rebuild the extra forever, so the main
    /// thread never handles events and the pointer stays a spinning cursor.
    @State private var menuBarInserted = UserDefaults.standard.bool(forKey: Pref.runInBackground)

    var body: some Scene {
        Window("Whoosh", id: "main") {
            RootView()
                .environment(model)
                .frame(minWidth: 900, minHeight: 640)
                .onAppear { model.start() }
                .onChange(of: runInBackground) { _, enabled in
                    menuBarInserted = enabled
                }
                .background {
                    SessionAnchor()
                }
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

        MenuBarExtra("Whoosh", systemImage: "arrow.left.arrow.right", isInserted: $menuBarInserted) {
            MenuBarMenu()
                .environment(model)
        }
        .menuBarExtraStyle(.menu)
    }
}

/// Keeps a way to reopen the main window after it has been closed.
private struct SessionAnchor: View {
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Color.clear
            .frame(width: 0, height: 0)
            .allowsHitTesting(false)
            .onAppear {
                Session.openMain = { openWindow(id: "main") }
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
    private var loginHideObserver: NSObjectProtocol?

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
        OfferNotifications.shared.prepare()
        let stayInBackground = UserDefaults.standard.bool(forKey: Pref.runInBackground)
        if stayInBackground && launchedAsLoginItem() {
            Session.pendingLoginHide = true
            NSApp.setActivationPolicy(.accessory)
            loginHideObserver = NotificationCenter.default.addObserver(
                forName: NSWindow.didBecomeKeyNotification,
                object: nil,
                queue: .main
            ) { note in
                // SwiftUI can open the window again after the first close. Keep
                // hiding it until the launch settle time, unless the user asks for it.
                guard Session.pendingLoginHide, let window = note.object as? NSWindow, window.canBecomeMain else { return }
                NSApp.setActivationPolicy(.accessory)
                DispatchQueue.main.async {
                    guard Session.pendingLoginHide else { return }
                    window.close()
                }
            }
            DispatchQueue.main.async { Session.hideLoginWindowIfNeeded() }
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) { Session.hideLoginWindowIfNeeded() }
            DispatchQueue.main.asyncAfter(deadline: .now() + 2.0) { [weak self] in
                self?.stopLoginHide()
            }
        } else {
            NSApp.activate()
        }
        if stayInBackground {
            OfferNotifications.shared.requestAuthorization()
        }
        MainActor.assumeIsolated {
            Snapshot.writeIfRequested()
        }
        // The window can close before its view appears on a login launch.
        // Starting here keeps the engine up while only the menu bar is showing.
        AppModel.shared?.start()
        DispatchQueue.global(qos: .utility).async {
            registerShareExtension()
        }
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool {
        if Session.quitting {
            return true
        }
        let stay = UserDefaults.standard.bool(forKey: Pref.runInBackground)
        if stay {
            NSApp.setActivationPolicy(.accessory)
        }
        return !stay
    }

    func applicationShouldHandleReopen(_ sender: NSApplication, hasVisibleWindows _: Bool) -> Bool {
        stopLoginHide()
        Session.reveal()
        return false
    }

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        Session.quitting = true
        AppModel.shared?.shutdown()
        return .terminateNow
    }

    func applicationWillTerminate(_ notification: Notification) {
        AppModel.shared?.shutdown()
    }

    func application(_ application: NSApplication, open urls: [URL]) {
        DispatchQueue.main.async { [weak self] in
            self?.stopLoginHide()
            Session.reveal()
            AppModel.accept(urls)
        }
    }

    private func stopLoginHide() {
        Session.pendingLoginHide = false
        if let loginHideObserver {
            NotificationCenter.default.removeObserver(loginHideObserver)
            self.loginHideObserver = nil
        }
    }

    /// SMAppService marks a login launch with kAEOpenApplication / keyAEPropData 'prdt' / 'lnch'.
    /// A manual open does not, so enabling login still shows the window when you open Whoosh yourself.
    private func launchedAsLoginItem() -> Bool {
        guard let event = NSAppleEventManager.shared().currentAppleEvent else { return false }
        guard event.eventClass == AEEventClass(0x61657674), event.eventID == AEEventID(0x6F617070) else { return false }
        guard let descriptor = event.paramDescriptor(forKeyword: AEKeyword(0x70726474)) else { return false }
        return descriptor.enumCodeValue == 0x6C6E6368 || descriptor.typeCodeValue == 0x6C6E6368
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
