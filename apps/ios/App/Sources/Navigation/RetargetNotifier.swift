import SdrmmCore
import UserNotifications
import os

protocol RetargetNotifying: AnyObject {
    func requestAuthorization() async
    func post(_ notice: RetargetNotice, distance: String)
}

final class RetargetNotifier: RetargetNotifying {
    private let center: UNUserNotificationCenter

    init(center: UNUserNotificationCenter = .current()) {
        self.center = center
    }

    func requestAuthorization() async {
        Log.nav.error("retarget alerts not built yet")
    }

    func post(_ notice: RetargetNotice, distance: String) {
        Log.nav.error("retarget alerts not built yet")
    }
}
