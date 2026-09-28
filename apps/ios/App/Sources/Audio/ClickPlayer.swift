import Foundation

protocol ClickPlaying: AnyObject {
    func start() throws
    func stop()
    func setStrength(_ strength: Float)
}

final class ClickPlayer: ClickPlaying {
    private let session: AudioSessionController

    init(session: AudioSessionController) {
        self.session = session
    }

    func start() throws {
        throw NotBuilt(feature: "Clicks")
    }

    func stop() {}

    func setStrength(_ strength: Float) {}
}
