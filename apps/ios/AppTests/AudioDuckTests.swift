import AVFAudio
import XCTest

@testable import SDRmm

@MainActor
final class AudioDuckTests: XCTestCase {
    private final class CategoryRecorder: AudioSessionPort {
        private(set) var categories: [(AVAudioSession.Mode, AVAudioSession.CategoryOptions)] = []
        var fail = false

        func setCategory(
            _ category: AVAudioSession.Category,
            mode: AVAudioSession.Mode,
            options: AVAudioSession.CategoryOptions
        ) throws {
            if fail {
                throw NotBuilt(feature: "Audio")
            }
            categories.append((mode, options))
        }

        func setActive(_ active: Bool, options: AVAudioSession.SetActiveOptions) throws {}
    }

    func testDuckSwitchesToVoicePromptAndBack() throws {
        let port = CategoryRecorder()
        let audio = AudioSessionController(session: port)
        try audio.duck(true)
        try audio.duck(false)
        XCTAssertEqual(port.categories.map(\.0), [.voicePrompt, .default])
        let ducked = try XCTUnwrap(port.categories.first?.1)
        XCTAssertTrue(ducked.contains(.duckOthers))
        XCTAssertTrue(ducked.contains(.interruptSpokenAudioAndMixWithOthers))
        XCTAssertEqual(port.categories.last?.1, [.mixWithOthers])
    }

    func testDuckFailureThrows() {
        let port = CategoryRecorder()
        port.fail = true
        XCTAssertThrowsError(try AudioSessionController(session: port).duck(true))
    }
}
