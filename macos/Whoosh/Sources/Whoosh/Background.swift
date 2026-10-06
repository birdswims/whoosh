import AppKit
import ServiceManagement
import SwiftUI
import UserNotifications

/// Menu-bar session. Both preferences stay off until the user turns them on,
/// so closing the window still quits.
enum Session {
    static var openMain: (() -> Void)?
    static var pendingLoginHide = false
    static var quitting = false

    static func windowOnScreen() -> Bool {
        NSApp.windows.contains { window in
            window.canBecomeMain && window.isVisible && !window.isMiniaturized
        }
    }

    static func reveal() {
        pendingLoginHide = false
        NSApp.setActivationPolicy(.regular)
        NSApp.unhide(nil)
        openMain?()
        NSApp.activate()
        DispatchQueue.main.async {
            openMain?()
            for window in NSApp.windows where window.canBecomeMain {
                window.makeKeyAndOrderFront(nil)
            }
            NSApp.activate()
        }
    }

    static func backgroundChanged(_ enabled: Bool) {
        if enabled {
            OfferNotifications.shared.prepare()
            OfferNotifications.shared.requestAuthorization()
            return
        }
        NSApp.setActivationPolicy(.regular)
        if !windowOnScreen() {
            reveal()
        }
    }

    static func announceOffer(_ offer: Offer) {
        guard UserDefaults.standard.bool(forKey: Pref.runInBackground), !windowOnScreen() else { return }
        OfferNotifications.shared.postOffer(offer)
    }

    static func announceNotice(_ text: String) {
        guard UserDefaults.standard.bool(forKey: Pref.runInBackground), !windowOnScreen() else { return }
        if text.hasPrefix("Copying from ") { return }
        OfferNotifications.shared.postNotice(text)
    }

    static func hideLoginWindowIfNeeded() {
        guard pendingLoginHide else { return }
        let windows = NSApp.windows.filter(\.canBecomeMain)
        guard !windows.isEmpty else { return }
        NSApp.setActivationPolicy(.accessory)
        for window in windows {
            window.close()
        }
    }
}

enum LoginItem {
    static var isOn: Bool {
        let status = SMAppService.mainApp.status
        return status == .enabled || status == .requiresApproval
    }

    static var note: String {
        SMAppService.mainApp.status == .requiresApproval
            ? "Allow Whoosh in System Settings, under General, Login Items."
            : ""
    }

    /// Nil when the login item matches `enabled`. A string is a failure the settings screen can show.
    static func update(_ enabled: Bool) -> String? {
        let service = SMAppService.mainApp
        do {
            if enabled {
                if service.status != .enabled && service.status != .requiresApproval {
                    try service.register()
                }
            } else if service.status == .enabled || service.status == .requiresApproval {
                try service.unregister()
            }
            UserDefaults.standard.set(enabled, forKey: Pref.startAtLogin)
            return nil
        } catch {
            return "Whoosh could not update Login Items."
        }
    }
}

struct MenuBarMenu: View {
    @Environment(AppModel.self) private var model
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Button("Open Whoosh") {
            Session.openMain = { openWindow(id: "main") }
            Session.reveal()
        }
        let sources = model.peers.filter(\.showsClipboard)
        if sources.isEmpty {
            Button("No trusted devices nearby") {}
                .disabled(true)
        } else {
            ForEach(sources) { peer in
                Button("Copy from \(peer.name.isEmpty ? "device" : peer.name)") {
                    model.copyFrom(peer)
                }
            }
        }
        Divider()
        Button("Quit Whoosh") {
            Session.quitting = true
            NSApp.terminate(nil)
        }
    }
}

final class OfferNotifications: NSObject, UNUserNotificationCenterDelegate {
    static let shared = OfferNotifications()

    private static let category = "whoosh.offer"
    private static let accept = "accept"
    private static let decline = "decline"

    func prepare() {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        let accept = UNNotificationAction(identifier: Self.accept, title: "Accept", options: [.foreground])
        let decline = UNNotificationAction(identifier: Self.decline, title: "Decline", options: [.destructive])
        let category = UNNotificationCategory(
            identifier: Self.category,
            actions: [accept, decline],
            intentIdentifiers: [],
            options: []
        )
        center.setNotificationCategories([category])
    }

    func requestAuthorization() {
        UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .sound]) { _, _ in }
    }

    func postOffer(_ offer: Offer) {
        let content = UNMutableNotificationContent()
        content.title = offer.peer
        var body = offer.requestSummary
        if offer.via == "quickshare", let pin = offer.pin, !pin.isEmpty {
            body += " Compare code \(pin)."
        }
        content.body = body
        content.categoryIdentifier = Self.category
        content.sound = .default
        let request = UNNotificationRequest(identifier: offer.id, content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }

    func postNotice(_ text: String) {
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        guard !trimmed.isEmpty else { return }
        let content = UNMutableNotificationContent()
        content.title = "Whoosh"
        content.body = String(trimmed.prefix(180))
        content.sound = .default
        let request = UNNotificationRequest(identifier: "notice-\(UUID().uuidString)", content: content, trigger: nil)
        UNUserNotificationCenter.current().add(request)
    }

    func clear(_ id: String) {
        let center = UNUserNotificationCenter.current()
        center.removeDeliveredNotifications(withIdentifiers: [id])
        center.removePendingNotificationRequests(withIdentifiers: [id])
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        let present = {
            if Session.windowOnScreen() {
                completionHandler([])
            } else {
                completionHandler([.banner, .sound])
            }
        }
        if Thread.isMainThread {
            present()
        } else {
            DispatchQueue.main.async(execute: present)
        }
    }

    func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let id = response.notification.request.identifier
        let action = response.actionIdentifier
        DispatchQueue.main.async {
            switch action {
            case Self.accept:
                AppModel.shared?.decide(id, accept: true)
                Session.reveal()
            case Self.decline:
                AppModel.shared?.decide(id, accept: false)
            case UNNotificationDismissActionIdentifier:
                break
            default:
                Session.reveal()
            }
            completionHandler()
        }
    }
}
