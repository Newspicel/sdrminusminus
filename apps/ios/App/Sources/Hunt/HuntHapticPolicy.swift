import Foundation
import SdrmmCore

struct HapticCueEvent: Equatable {
    let id: Int
    let cue: HapticCue
}

nonisolated enum HapticCue: Equatable, Sendable {
    case increase, decrease, success
}

nonisolated struct HuntHapticPolicy {
    static let minimumInterval: TimeInterval = 0.7
    static let successStrength: Float = 0.9
    static let rearmStrength: Float = 0.8

    private var lastTrend: Trend?
    private var armed = true
    private var lastCue: TimeInterval?

    mutating func cue(trend: Trend, strength: Float, at time: TimeInterval) -> HapticCue? {
        let previous = lastTrend
        lastTrend = trend
        if strength < Self.rearmStrength {
            armed = true
        }
        let candidate = next(trend: trend, previous: previous, strength: strength)
        guard let candidate, allowed(at: time) else {
            return nil
        }
        if candidate == .success {
            armed = false
        }
        lastCue = time
        return candidate
    }

    mutating func reset() {
        self = HuntHapticPolicy()
    }

    private func next(trend: Trend, previous: Trend?, strength: Float) -> HapticCue? {
        if armed, strength.isFinite, strength >= Self.successStrength {
            return .success
        }
        guard trend != previous else {
            return nil
        }
        switch trend {
        case .warmer: return .increase
        case .colder: return .decrease
        case .waiting, .onTop: return nil
        }
    }

    private func allowed(at time: TimeInterval) -> Bool {
        guard let lastCue else {
            return true
        }
        return time - lastCue >= Self.minimumInterval
    }
}
