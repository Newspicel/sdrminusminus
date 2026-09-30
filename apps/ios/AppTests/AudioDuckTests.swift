import AVFAudio
import Synchronization
import XCTest

@testable import SDRmm

@MainActor
final class AudioDuckTests: XCTestCase {
    private final class CategoryRecorder: AudioSessionPort {
        private struct State {
            var categories: [(AVAudioSession.Mode, AVAudioSession.CategoryOptions)] = []
            var fail = false
        }

        private let state = Mutex(State())

        var categories: [(AVAudioSession.Mode, AVAudioSession.CategoryOptions)] {
            state.withLock { $0.categories }
        }

        var fail: Bool {
            get { state.withLock { $0.fail } }
            set { state.withLock { $0.fail = newValue } }
        }

        func setCategory(
            _ category: AVAudioSession.Category,
            mode: AVAudioSession.Mode,
            options: AVAudioSession.CategoryOptions
        ) throws {
            let refused = state.withLock { state in
                if !state.fail {
                    state.categories.append((mode, options))
                }
                return state.fail
            }
            if refused {
                throw NotBuilt(feature: "Audio")
            }
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
