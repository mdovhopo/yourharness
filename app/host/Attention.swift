import AppKit
import HarnessBackend
@preconcurrency import UserNotifications

/// Notifications and the Dock badge for what the backend wants you to notice. Nothing is
/// posted while you're looking at the app; clicking one opens its thread. `YH_NOTIFICATIONS=log`
/// prints them instead, so a test run never asks for permission.
@MainActor
final class AttentionCenter: NSObject, UNUserNotificationCenterDelegate {
    var open: (String) -> Void = { _ in }
    private var asked = false

    func handle(_ attention: Attention) {
        switch attention {
        case .badge(let count):
            if env["YH_NOTIFICATIONS"] == "log" { return log("BADGE \(count)") }
            NSApp.dockTile.badgeLabel = count > 0 ? "\(count)" : nil
        case .notify(let thread, let title, let body):
            if env["YH_NOTIFICATIONS"] == "log" { return log("NOTIFY \(title): \(body)") }
            guard !NSApp.isActive else { return }
            post(thread: thread, title: title, body: body)
        }
    }

    private func post(thread: String, title: String, body: String) {
        let center = UNUserNotificationCenter.current()
        center.delegate = self
        let content = UNMutableNotificationContent()
        content.title = title
        content.body = body
        content.userInfo = ["thread": thread]
        let request = UNNotificationRequest(identifier: thread, content: content, trigger: nil)
        if asked {
            center.add(request)
        } else {
            // Asked the first time there's something to say, not at launch.
            asked = true
            center.requestAuthorization(options: [.alert, .sound, .badge]) { granted, _ in
                if granted { center.add(request) }
            }
        }
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        let thread = response.notification.request.content.userInfo["thread"] as? String
        await MainActor.run {
            NSApp.activate()
            if let thread { self.open(thread) }
        }
    }
}
