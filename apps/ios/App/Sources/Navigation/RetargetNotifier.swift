import SdrmmCore
import UserNotifications
import os

protocol RetargetNotifying: AnyObject {
    func requestAuthorization() async -> Bool
    func post(_ notice: RetargetNotice, distance: String)
    func takeFailure() -> String?
}

nonisolated enum RetargetText {
    static let title = "New target"
    static let thread = "retarget"

    static func kind(_ kind: GuidanceKind) -> String {
        switch kind {
        case .probe: "Cross"
        case .estimate: "Target"
        }
    }

    static func body(_ notice: RetargetNotice, distance: String) -> String {
        "\(distance) \u{00B7} \(kind(notice.target.kind))"
    }

    static func identifier(_ notice: RetargetNotice) -> String {
        "retarget.\(notice.mission)"
    }
}

final class RetargetNotifier: RetargetNotifying {
    private let center: UNUserNotificationCenter
    private var failure: String?

    init(center: UNUserNotificationCenter = .current()) {
        self.center = center
    }

    func requestAuthorization() async -> Bool {
        do {
            return try await center.requestAuthorization(options: [.alert, .sound])
        } catch {
            Log.nav.error("alerts: \(error.localizedDescription, privacy: .public)")
            return false
        }
    }

    func post(_ notice: RetargetNotice, distance: String) {
        let content = UNMutableNotificationContent()
        content.title = RetargetText.title
        content.body = RetargetText.body(notice, distance: distance)
        content.sound = .default
        content.userInfo = ["mission": notice.mission]
        content.threadIdentifier = RetargetText.thread
        let request = UNNotificationRequest(
            identifier: RetargetText.identifier(notice),
            content: content,
            trigger: nil
        )
        let center = center
        Task { [weak self] in
            do {
                try await center.add(request)
            } catch {
                Log.nav.error("alert failed: \(error.localizedDescription, privacy: .public)")
                self?.failure = error.localizedDescription
            }
        }
    }

    func takeFailure() -> String? {
        defer { failure = nil }
        return failure
    }
}
