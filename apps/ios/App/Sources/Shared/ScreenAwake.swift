import SwiftUI
import UIKit

final class ScreenAwake {
    private(set) var awake = false

    func update(missionOpen: Bool, phase: ScenePhase) {
        awake = missionOpen && phase == .active
        UIApplication.shared.isIdleTimerDisabled = awake
    }
}
