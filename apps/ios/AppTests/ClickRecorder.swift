@testable import SDRmm

@MainActor
final class ClickRecorder: ClickPlaying {
    private(set) var starts = 0
    private(set) var stops = 0
    private(set) var strengths: [Float] = []

    func start() throws {
        starts += 1
    }

    func stop() {
        stops += 1
    }

    func setStrength(_ strength: Float) {
        strengths.append(strength)
    }
}
