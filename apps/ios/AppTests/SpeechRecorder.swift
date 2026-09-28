@testable import SDRmm

@MainActor
final class SpeechRecorder: SpeechPrompting {
    private(set) var spoken: [(text: String, urgent: Bool)] = []
    private(set) var stops = 0

    func say(_ text: String, urgent: Bool) {
        spoken.append((text, urgent))
    }

    func stop() {
        stops += 1
    }
}
