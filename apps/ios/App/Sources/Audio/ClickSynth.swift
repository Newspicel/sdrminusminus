import AVFAudio
import Foundation
import Synchronization

nonisolated enum ClickShape {
    static let slowestHz: Double = 1.5
    static let fastestHz: Double = 40
    static let toneHz: Double = 1_800
    static let peak: Float = 0.28
    static let clickSeconds: Double = 0.004
    static let floor: Double = 1e-4

    static var decaySeconds: Double {
        clickSeconds / log(Double(peak) / floor)
    }

    static func rateHz(strength: Float) -> Double {
        let clamped = strength.isFinite ? Double(min(max(strength, 0), 1)) : 0
        return slowestHz + (fastestHz - slowestHz) * clamped * clamped
    }
}

nonisolated struct ClickState {
    var samplesToNext: Double = 0
    var clickPos = -1
    var amplitude: Float = 0
    var phase: Double = 0
}

nonisolated final class ClickSynth: @unchecked Sendable {
    let sampleRate: Double
    private let strength = Atomic<UInt32>(Float(0).bitPattern)
    private let state: UnsafeMutablePointer<ClickState>
    private let decay: Float
    private let clickLength: Int
    private let phaseStep: Double

    init(sampleRate: Double) {
        self.sampleRate = sampleRate
        decay = Float(exp(-1 / (ClickShape.decaySeconds * sampleRate)))
        clickLength = Int((ClickShape.clickSeconds * sampleRate).rounded())
        phaseStep = 2 * Double.pi * ClickShape.toneHz / sampleRate
        state = .allocate(capacity: 1)
        state.initialize(to: ClickState())
    }

    deinit {
        state.deinitialize(count: 1)
        state.deallocate()
    }

    func setStrength(_ strength: Float) {
        self.strength.store(strength.bitPattern, ordering: .relaxed)
    }

    func render(into buffer: UnsafeMutablePointer<Float>, frames: Int) {
        let level = Float(bitPattern: strength.load(ordering: .relaxed))
        let period = sampleRate / ClickShape.rateHz(strength: level)
        var current = state.pointee
        current.samplesToNext = min(current.samplesToNext, period)
        for index in 0..<frames {
            if current.clickPos < 0, current.samplesToNext <= 0 {
                current.clickPos = 0
                current.amplitude = ClickShape.peak
                current.phase = 0
                current.samplesToNext += period
            }
            var sample: Float = 0
            if current.clickPos >= 0 {
                sample = current.amplitude * Float(sin(current.phase))
                current.phase += phaseStep
                current.amplitude *= decay
                current.clickPos += 1
                if current.clickPos >= clickLength {
                    current.clickPos = -1
                }
            }
            current.samplesToNext -= 1
            buffer[index] = sample
        }
        state.pointee = current
    }

    func sourceNode(format: AVAudioFormat) -> AVAudioSourceNode {
        AVAudioSourceNode(format: format) { [self] _, _, frameCount, audioBufferList in
            let buffers = UnsafeMutableAudioBufferListPointer(audioBufferList)
            if let data = buffers.first?.mData {
                render(into: data.assumingMemoryBound(to: Float.self), frames: Int(frameCount))
            }
            return noErr
        }
    }
}
