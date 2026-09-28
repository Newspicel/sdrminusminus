import AVFAudio
import Foundation

protocol SpeechPrompting: AnyObject {
    func say(_ text: String, urgent: Bool)
    func stop()
}

final class SpeechPrompter: NSObject, SpeechPrompting {
    private let settings: SettingsStore
    private let session: AudioSessionController
    private let synthesizer = AVSpeechSynthesizer()

    init(settings: SettingsStore, session: AudioSessionController) {
        self.settings = settings
        self.session = session
    }

    func say(_ text: String, urgent: Bool) {
        guard settings.voiceOn else {
            return
        }
        if urgent {
            synthesizer.stopSpeaking(at: .word)
        }
        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = settings.voiceID.flatMap(AVSpeechSynthesisVoice.init(identifier:))
        utterance.prefersAssistiveTechnologySettings = true
        synthesizer.speak(utterance)
    }

    func stop() {
        synthesizer.stopSpeaking(at: .immediate)
    }
}
