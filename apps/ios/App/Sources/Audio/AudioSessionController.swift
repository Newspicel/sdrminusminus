import AVFAudio
import os

protocol AudioSessionPort: AnyObject {
    func setCategory(
        _ category: AVAudioSession.Category,
        mode: AVAudioSession.Mode,
        options: AVAudioSession.CategoryOptions
    ) throws
    func setActive(_ active: Bool, options: AVAudioSession.SetActiveOptions) throws
}

extension AVAudioSession: AudioSessionPort {}

final class AudioSessionController {
    var onInterruptionEnded: (@MainActor () -> Void)?
    private let session: any AudioSessionPort

    init(session: any AudioSessionPort = AVAudioSession.sharedInstance()) {
        self.session = session
    }

    func activate() throws {
        try session.setCategory(.playback, mode: .default, options: [.mixWithOthers])
        try session.setActive(true, options: [])
    }

    func deactivate() {
        do {
            try session.setActive(false, options: .notifyOthersOnDeactivation)
        } catch {
            Log.audio.error("audio session off: \(error.localizedDescription, privacy: .public)")
        }
    }
}
