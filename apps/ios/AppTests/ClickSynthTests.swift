import AVFAudio
import Synchronization
import XCTest

@testable import SDRmm

final class ClickSynthTests: XCTestCase {
    private let rate = 48_000.0

    private func render(strength: Float, seconds: Double, chunk: Int = 512) -> [Float] {
        let synth = ClickSynth(sampleRate: rate)
        synth.setStrength(strength)
        let total = Int(rate * seconds)
        var samples = [Float](repeating: 0, count: total)
        samples.withUnsafeMutableBufferPointer { buffer in
            guard let base = buffer.baseAddress else {
                return
            }
            var done = 0
            while done < total {
                let frames = min(chunk, total - done)
                synth.render(into: base + done, frames: frames)
                done += frames
            }
        }
        return samples
    }

    private func runs(_ samples: [Float]) -> [Int] {
        var lengths: [Int] = []
        var current = 0
        for sample in samples {
            if sample != 0 {
                current += 1
            } else if current > 0 {
                lengths.append(current)
                current = 0
            }
        }
        if current > 0 {
            lengths.append(current)
        }
        return lengths
    }

    func testRateCurve() {
        XCTAssertEqual(ClickShape.rateHz(strength: 0), 1.5, accuracy: 1e-9)
        XCTAssertEqual(ClickShape.rateHz(strength: 1), 40, accuracy: 1e-9)
        XCTAssertEqual(ClickShape.rateHz(strength: 0.5), 11.125, accuracy: 1e-9)
        XCTAssertEqual(ClickShape.rateHz(strength: .nan), 1.5, accuracy: 1e-9)
        XCTAssertEqual(ClickShape.rateHz(strength: 3), 40, accuracy: 1e-9)
        XCTAssertEqual(ClickShape.rateHz(strength: -1), 1.5, accuracy: 1e-9)
        XCTAssertEqual(ClickShape.decaySeconds, 5.04e-4, accuracy: 1e-6)
    }

    func testFullStrengthSecondHasFortyClicks() {
        let onsets = runs(render(strength: 1, seconds: 1)).count
        XCTAssertTrue((39...41).contains(onsets), "\(onsets)")
    }

    func testZeroStrengthSecondHasOneOrTwoClicks() {
        let onsets = runs(render(strength: 0, seconds: 1)).count
        XCTAssertTrue((1...2).contains(onsets), "\(onsets)")
    }

    func testPeakAmplitude() {
        let samples = render(strength: 1, seconds: 0.5)
        let peak = samples.map { abs($0) }.max() ?? 0
        XCTAssertLessThanOrEqual(peak, 0.28)
        XCTAssertGreaterThan(peak, 0.2)
    }

    func testClickLength() {
        let lengths = runs(render(strength: 1, seconds: 1, chunk: 333))
        XCTAssertFalse(lengths.isEmpty)
        for length in lengths {
            XCTAssertTrue((191...193).contains(length), "\(length)")
        }
    }

    func testStrengthChangeSpeedsUpClicks() {
        let synth = ClickSynth(sampleRate: rate)
        var samples = [Float](repeating: 0, count: Int(rate))
        samples.withUnsafeMutableBufferPointer { buffer in
            guard let base = buffer.baseAddress else {
                return
            }
            synth.render(into: base, frames: buffer.count / 2)
            synth.setStrength(1)
            synth.render(into: base + buffer.count / 2, frames: buffer.count / 2)
        }
        let early = runs(Array(samples[..<(samples.count / 2)])).count
        let late = runs(Array(samples[(samples.count / 2)...])).count
        XCTAssertLessThanOrEqual(early, 1)
        XCTAssertGreaterThanOrEqual(late, 18)
    }

    func testSourceNodeRendersClicksInAnEngine() throws {
        guard let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 1),
            let buffer = AVAudioPCMBuffer(pcmFormat: format, frameCapacity: 4_800)
        else {
            return XCTFail("no format")
        }
        let synth = ClickSynth(sampleRate: rate)
        synth.setStrength(1)
        let engine = AVAudioEngine()
        try engine.enableManualRenderingMode(.offline, format: format, maximumFrameCount: 4_800)
        let source = synth.sourceNode(format: format)
        engine.attach(source)
        engine.connect(source, to: engine.mainMixerNode, format: format)
        try engine.start()
        defer { engine.stop() }
        XCTAssertEqual(try engine.renderOffline(4_800, to: buffer), .success)
        guard let channel = buffer.floatChannelData?[0] else {
            return XCTFail("no samples")
        }
        let samples = Array(UnsafeBufferPointer(start: channel, count: Int(buffer.frameLength)))
        XCTAssertEqual(runs(samples).count, 4)
    }

    func testShouldResumeNeedsEndedAndResume() {
        let ended = AVAudioSession.InterruptionType.ended.rawValue
        let began = AVAudioSession.InterruptionType.began.rawValue
        let resume = AVAudioSession.InterruptionOptions.shouldResume.rawValue
        XCTAssertTrue(
            AudioSessionController.shouldResume([
                AVAudioSessionInterruptionTypeKey: ended, AVAudioSessionInterruptionOptionKey: resume,
            ])
        )
        XCTAssertFalse(AudioSessionController.shouldResume([AVAudioSessionInterruptionTypeKey: ended]))
        XCTAssertFalse(
            AudioSessionController.shouldResume([
                AVAudioSessionInterruptionTypeKey: began, AVAudioSessionInterruptionOptionKey: resume,
            ])
        )
        XCTAssertFalse(AudioSessionController.shouldResume(nil))
    }

    @MainActor
    func testActivateOffMainThenDuckAndBack() async throws {
        let session = CategoryRecorder()
        let controller = AudioSessionController(session: session, center: NotificationCenter())
        let outcome = Outcomes()
        controller.activate { outcome.values.append($0) }
        await eventually { outcome.values.count == 1 }
        XCTAssertEqual(outcome.values, [nil])
        XCTAssertEqual(session.activations, [true])
        XCTAssertFalse(session.activatedOnMain)
        try controller.duck(true)
        try controller.duck(false)
        XCTAssertEqual(session.modes, [.default, .voicePrompt, .default])
        XCTAssertEqual(
            session.options,
            [
                AudioSessionController.playOptions, AudioSessionController.duckOptions,
                AudioSessionController.playOptions,
            ]
        )
        controller.deactivate()
        await eventually { session.activations == [true, false] }
        XCTAssertEqual(session.activations, [true, false])
    }

    @MainActor
    func testActivationFailureIsReported() async {
        let session = CategoryRecorder()
        session.refuse = true
        let controller = AudioSessionController(session: session, center: NotificationCenter())
        let outcome = Outcomes()
        controller.activate { outcome.values.append($0) }
        await eventually { outcome.values.count == 1 }
        guard let failure = outcome.values.first ?? nil else {
            return XCTFail("activation did not fail")
        }
        XCTAssertEqual(CoreErrorText.short(failure), "Audio off")
        XCTAssertTrue(session.activations.isEmpty)
    }

    @MainActor
    func testInterruptionEndedCallsBack() async {
        let center = NotificationCenter()
        let controller = AudioSessionController(session: CategoryRecorder(), center: center)
        var resumed = 0
        controller.onInterruptionEnded = { resumed += 1 }
        center.post(
            name: AVAudioSession.interruptionNotification,
            object: nil,
            userInfo: [
                AVAudioSessionInterruptionTypeKey: AVAudioSession.InterruptionType.ended.rawValue,
                AVAudioSessionInterruptionOptionKey: AVAudioSession.InterruptionOptions.shouldResume.rawValue,
            ]
        )
        await eventually { resumed == 1 }
        XCTAssertEqual(resumed, 1)
    }
}

@MainActor
final class Outcomes {
    var values: [AudioOff?] = []
}

final class CategoryRecorder: AudioSessionPort {
    private struct State {
        var modes: [AVAudioSession.Mode] = []
        var options: [AVAudioSession.CategoryOptions] = []
        var activations: [Bool] = []
        var activatedOnMain = false
        var refuse = false
    }

    private let state = Mutex(State())

    var modes: [AVAudioSession.Mode] { state.withLock { $0.modes } }
    var options: [AVAudioSession.CategoryOptions] { state.withLock { $0.options } }
    var activations: [Bool] { state.withLock { $0.activations } }
    var activatedOnMain: Bool { state.withLock { $0.activatedOnMain } }

    var refuse: Bool {
        get { state.withLock { $0.refuse } }
        set { state.withLock { $0.refuse = newValue } }
    }

    func setCategory(
        _ category: AVAudioSession.Category,
        mode: AVAudioSession.Mode,
        options: AVAudioSession.CategoryOptions
    ) throws {
        state.withLock { state in
            state.modes.append(mode)
            state.options.append(options)
        }
    }

    func setActive(_ active: Bool, options: AVAudioSession.SetActiveOptions) throws {
        let onMain = Thread.isMainThread
        let refused = state.withLock { state in
            state.activatedOnMain = state.activatedOnMain || onMain
            if state.refuse {
                return true
            }
            state.activations.append(active)
            return false
        }
        if refused {
            throw AudioOff(reason: "refused")
        }
    }
}
