@testable import SDRmm

@MainActor
final class ClickRecorder: ClickPlaying {
    var onFailure: (@MainActor (Error) -> Void)?
    var failStart: Error?
    private(set) var starts = 0
    private(set) var stops = 0
    private(set) var strengths: [Float] = []
    private(set) var running = false

    func start() throws {
        if let failStart {
            throw failStart
        }
        starts += 1
        running = true
    }

    func stop() {
        stops += 1
        running = false
    }

    func setStrength(_ strength: Float) {
        strengths.append(strength)
    }

    func fail(_ error: Error) {
        onFailure?(error)
    }
}
