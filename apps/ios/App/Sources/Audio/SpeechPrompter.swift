import AVFAudio
import Foundation
import os

protocol SpeechPrompting: AnyObject {
    func say(_ text: String, urgent: Bool)
    func stop()
}

nonisolated struct SpeechQueue: Equatable {
    static let staleS: TimeInterval = 5

    struct Item: Equatable {
        let text: String
        let queuedAt: Date
    }

    private(set) var waiting: [Item] = []
    private(set) var speaking = false

    var idle: Bool { !speaking && waiting.isEmpty }

    mutating func interrupt() {
        waiting.removeAll()
        speaking = true
    }

    mutating func enqueue(_ text: String, at now: Date) -> [String] {
        let stale = waiting.filter { now.timeIntervalSince($0.queuedAt) > Self.staleS }.map(\.text)
        waiting.removeAll { now.timeIntervalSince($0.queuedAt) > Self.staleS }
        waiting.append(Item(text: text, queuedAt: now))
        return stale
    }

    mutating func next() -> String? {
        guard !speaking, !waiting.isEmpty else {
            return nil
        }
        speaking = true
        return waiting.removeFirst().text
    }

    mutating func finished() {
        speaking = false
    }

    mutating func clear() {
        waiting.removeAll()
        speaking = false
    }
}

final class SpeechPrompter: NSObject, SpeechPrompting, AVSpeechSynthesizerDelegate {
    private let settings: SettingsStore
    private let session: AudioSessionController
    private let synthesizer = AVSpeechSynthesizer()
    private var queue = SpeechQueue()
    private var ducked = false
    private var current: AVSpeechUtterance?

    init(settings: SettingsStore, session: AudioSessionController) {
        self.settings = settings
        self.session = session
        super.init()
        synthesizer.delegate = self
    }

    func say(_ text: String, urgent: Bool) {
        guard settings.voiceOn else {
            return
        }
        if urgent {
            queue.interrupt()
            current = nil
            synthesizer.stopSpeaking(at: .word)
            speak(text)
            return
        }
        for stale in queue.enqueue(text, at: Date()) {
            Log.audio.debug("dropped stale prompt: \(stale, privacy: .private)")
        }
        speakNext()
    }

    func stop() {
        queue.clear()
        current = nil
        synthesizer.stopSpeaking(at: .immediate)
        release()
    }

    private func speakNext() {
        guard let text = queue.next() else {
            if queue.idle {
                release()
            }
            return
        }
        speak(text)
    }

    private func speak(_ text: String) {
        let utterance = AVSpeechUtterance(string: text)
        utterance.voice = voice()
        utterance.rate = AVSpeechUtteranceDefaultSpeechRate
        utterance.prefersAssistiveTechnologySettings = true
        current = utterance
        synthesizer.speak(utterance)
    }

    private func voice() -> AVSpeechSynthesisVoice? {
        if let id = settings.voiceID, let chosen = AVSpeechSynthesisVoice(identifier: id) {
            return chosen
        }
        return AVSpeechSynthesisVoice(language: Locale.current.language.languageCode?.identifier)
    }

    private func isCurrent(_ id: ObjectIdentifier) -> Bool {
        current.map { ObjectIdentifier($0) == id } ?? false
    }

    private func started(_ id: ObjectIdentifier) {
        guard isCurrent(id), !ducked else {
            return
        }
        ducked = true
        do {
            try session.duck(true)
        } catch {
            Log.audio.error("duck failed: \(error.localizedDescription, privacy: .public)")
        }
    }

    private func ended(_ id: ObjectIdentifier) {
        guard isCurrent(id) else {
            if current == nil, queue.idle {
                release()
            }
            return
        }
        current = nil
        queue.finished()
        speakNext()
    }

    private func release() {
        guard ducked else {
            return
        }
        ducked = false
        do {
            try session.duck(false)
        } catch {
            Log.audio.error("unduck failed: \(error.localizedDescription, privacy: .public)")
        }
    }

    nonisolated func speechSynthesizer(
        _ synthesizer: AVSpeechSynthesizer,
        didStart utterance: AVSpeechUtterance
    ) {
        let id = ObjectIdentifier(utterance)
        Task { @MainActor [weak self] in self?.started(id) }
    }

    nonisolated func speechSynthesizer(
        _ synthesizer: AVSpeechSynthesizer,
        didFinish utterance: AVSpeechUtterance
    ) {
        let id = ObjectIdentifier(utterance)
        Task { @MainActor [weak self] in self?.ended(id) }
    }

    nonisolated func speechSynthesizer(
        _ synthesizer: AVSpeechSynthesizer,
        didCancel utterance: AVSpeechUtterance
    ) {
        let id = ObjectIdentifier(utterance)
        Task { @MainActor [weak self] in self?.ended(id) }
    }
}
