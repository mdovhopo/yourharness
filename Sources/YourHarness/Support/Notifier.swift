import AppKit
import UserNotifications

/// macOS notifications for threads you aren't looking at: a turn finished, or Claude is
/// waiting for you. Clicking one opens its thread. Each thread has at most one of each
/// kind on screen; a newer one replaces it.
@MainActor
final class Notifier: NSObject, UNUserNotificationCenterDelegate {
    enum Kind: String {
        case finished, needsYou
    }

    /// Opens a thread, for a clicked notification.
    var open: ((UUID) -> Void)?

    /// Nil outside an app bundle (`swift run`), where the notification center isn't available.
    private let center: UNUserNotificationCenter?
    /// Development: print notifications instead of posting them, so a test instance never
    /// asks you for permission.
    private let logsOnly = ProcessInfo.processInfo.environment["YH_NOTIFICATIONS"] == "log"

    override init() {
        center = Bundle.main.bundleIdentifier == nil ? nil : UNUserNotificationCenter.current()
        super.init()
        if !logsOnly { center?.delegate = self }
    }

    func post(_ kind: Kind, thread: UUID, title: String, subtitle: String, body: String) {
        if logsOnly {
            Self.log("notification \(kind.rawValue): \(title) · \(subtitle) · \(body)")
            return
        }
        guard let center else { return }
        let content = UNMutableNotificationContent()
        content.title = title
        content.subtitle = subtitle
        content.body = body
        content.sound = .default
        content.threadIdentifier = thread.uuidString
        content.userInfo = ["thread": thread.uuidString]
        let request = UNNotificationRequest(identifier: Self.identifier(kind, thread), content: content, trigger: nil)
        Task {
            // Asked the first time there's something to say, not at launch.
            let settings = await center.notificationSettings()
            if settings.authorizationStatus == .notDetermined {
                guard (try? await center.requestAuthorization(options: [.alert, .sound])) == true else { return }
            } else if settings.authorizationStatus != .authorized && settings.authorizationStatus != .provisional {
                return
            }
            try? await center.add(request)
        }
    }

    /// The number on the Dock icon; 0 clears it.
    func setBadge(_ count: Int) {
        if logsOnly { Self.log("badge \(count)") }
        NSApp.dockTile.badgeLabel = count > 0 ? "\(count)" : nil
    }

    /// You're looking at the thread now: its notifications have done their job.
    func clear(thread: UUID) {
        center?.removeDeliveredNotifications(withIdentifiers: [Self.identifier(.finished, thread), Self.identifier(.needsYou, thread)])
    }

    /// Notifications were turned off for the app in System Settings.
    func isDenied() async -> Bool {
        guard let center, !logsOnly else { return false }
        return await center.notificationSettings().authorizationStatus == .denied
    }

    /// Unbuffered, so a test instance's log is complete even when it's killed.
    private static func log(_ line: String) {
        FileHandle.standardError.write(Data((line + "\n").utf8))
    }

    private static func identifier(_ kind: Kind, _ thread: UUID) -> String {
        "\(thread.uuidString).\(kind.rawValue)"
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse) async {
        guard let id = (response.notification.request.content.userInfo["thread"] as? String).flatMap(UUID.init(uuidString:)) else { return }
        await MainActor.run { open?(id) }
    }

    /// Posted while the app is in front only when it's about another thread, so show it.
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification) async -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }
}
