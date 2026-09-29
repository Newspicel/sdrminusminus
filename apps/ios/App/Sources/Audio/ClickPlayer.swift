import AVFAudio
import Foundation
import os

protocol ClickPlaying: AnyObject {
    var onFailure: (@MainActor (Error) -> Void)? { get set }
    func start() throws
    func stop()
    func setStrength(_ strength: Float)
}

final class ClickPlayer: ClickPlaying {
    var onFailure: (@MainActor (Error) -> Void)?
    private let center: NotificationCenter
    private var engine: AVAudioEngine?
    private var synth: ClickSynth?
    private var changes: ObserverToken?
    private var strength: Float = 0

    init(center: NotificationCenter = .default) {
        self.center = center
    }

    func start() throws {
        guard engine == nil else {
            return
        }
        try build()
    }

    func stop() {
        guard let engine else {
            return
        }
        changes = nil
        engine.stop()
        self.engine = nil
        synth = nil
        Log.audio.info("clicks off")
    }

    func setStrength(_ strength: Float) {
        self.strength = strength
        synth?.setStrength(strength)
    }

    private func build() throws {
        let engine = AVAudioEngine()
        let rate = engine.outputNode.outputFormat(forBus: 0).sampleRate
        guard rate > 0, let format = AVAudioFormat(standardFormatWithSampleRate: rate, channels: 1) else {
            throw AudioOff(reason: "No audio output")
        }
        let synth = ClickSynth(sampleRate: rate)
        synth.setStrength(strength)
        let source = synth.sourceNode(format: format)
        engine.attach(source)
        engine.connect(source, to: engine.mainMixerNode, format: format)
        engine.mainMixerNode.outputVolume = 1
        engine.prepare()
        do {
            try engine.start()
        } catch {
            throw AudioOff(reason: error.localizedDescription)
        }
        self.engine = engine
        self.synth = synth
        Log.audio.info("clicks on at \(Int(rate)) Hz")
        changes = ObserverToken(
            center: center,
            name: .AVAudioEngineConfigurationChange,
            object: engine
        ) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.rebuild()
            }
        }
    }

    private func rebuild() {
        guard engine != nil else {
            return
        }
        Log.audio.info("audio route changed, restarting clicks")
        stop()
        do {
            try build()
        } catch {
            Log.audio.error("clicks restart failed: \(error.localizedDescription, privacy: .public)")
            onFailure?(error)
        }
    }
}
