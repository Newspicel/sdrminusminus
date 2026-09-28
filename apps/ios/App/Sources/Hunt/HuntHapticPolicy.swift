import Foundation

struct HapticCueEvent: Equatable {
    let id: Int
    let cue: HapticCue
}

nonisolated enum HapticCue: Equatable, Sendable {
    case increase, decrease, success
}
