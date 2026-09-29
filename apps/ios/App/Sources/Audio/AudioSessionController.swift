import AVFAudio
import Foundation
import os

nonisolated protocol AudioSessionPort: AnyObject, Sendable {
    func setCategory(
        _ category: AVAudioSession.Category,
        mode: AVAudioSession.Mode,
        options: AVAudioSession.CategoryOptions
    ) throws
    func setActive(_ active: Bool, options: AVAudioSession.SetActiveOptions) throws
}

extension AVAudioSession: AudioSessionPort {}

nonisolated struct AudioOff: Error, Equatable {
    let reason: String
}

nonisolated final class ObserverToken: @unchecked Sendable {
    private let center: NotificationCenter
    private let token: any NSObjectProtocol

    init(
        center: NotificationCenter,
        name: Notification.Name,
        object: AnyObject?,
        block: @escaping @Sendable (Notification) -> Void
    ) {
        self.center = center
        token = center.addObserver(forName: name, object: object, queue: .main, using: block)
    }

    deinit {
        center.removeObserver(token)
    }
}

final class AudioSessionController {
    nonisolated static let playOptions: AVAudioSession.CategoryOptions = [.mixWithOthers]
    nonisolated static let duckOptions: AVAudioSession.CategoryOptions = [
        .mixWithOthers, .duckOthers, .interruptSpokenAudioAndMixWithOthers,
    ]

    var onInterruptionEnded: (@MainActor () -> Void)?
    private let session: any AudioSessionPort
    private let queue = DispatchQueue(label: "dev.newspicel.sdrmm.audio", qos: .userInitiated)
    private var interruptions: ObserverToken?

    init(
        session: any AudioSessionPort = AVAudioSession.sharedInstance(),
        center: NotificationCenter = .default
    ) {
        self.session = session
        interruptions = ObserverToken(
            center: center,
            name: AVAudioSession.interruptionNotification,
            object: nil
        ) { [weak self] notification in
            let resume = Self.shouldResume(notification.userInfo)
            MainActor.assumeIsolated {
                if resume {
                    self?.onInterruptionEnded?()
                }
            }
        }
    }

    func activate(_ done: @escaping @MainActor @Sendable (AudioOff?) -> Void) {
        let session = session
        queue.async {
            let failure: AudioOff?
            do {
                try session.setCategory(.playback, mode: .default, options: Self.playOptions)
                try session.setActive(true, options: [])
                failure = nil
            } catch {
                failure = AudioOff(reason: error.localizedDescription)
            }
            Task { @MainActor in done(failure) }
        }
    }

    func duck(_ on: Bool) throws {
        if on {
            try session.setCategory(.playback, mode: .voicePrompt, options: Self.duckOptions)
        } else {
            try session.setCategory(.playback, mode: .default, options: Self.playOptions)
        }
    }

    func deactivate() {
        let session = session
        queue.async {
            do {
                try session.setActive(false, options: .notifyOthersOnDeactivation)
            } catch {
                Log.audio.error("audio session off: \(error.localizedDescription, privacy: .public)")
            }
        }
    }

    nonisolated static func shouldResume(_ info: [AnyHashable: Any]?) -> Bool {
        guard let raw = info?[AVAudioSessionInterruptionTypeKey] as? UInt,
            AVAudioSession.InterruptionType(rawValue: raw) == .ended,
            let options = info?[AVAudioSessionInterruptionOptionKey] as? UInt
        else {
            return false
        }
        return AVAudioSession.InterruptionOptions(rawValue: options).contains(.shouldResume)
    }
}
