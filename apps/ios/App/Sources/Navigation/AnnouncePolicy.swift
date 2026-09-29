import Foundation

nonisolated struct Prompt: Equatable, Sendable {
    enum Stage: Equatable, Sendable {
        case far, near
    }

    let step: Int
    let stage: Stage
    let toNextM: Double
}

nonisolated struct AnnouncePolicy {
    private static let nearGapM = 50.0

    private var planID: UUID?
    private var spokenFar: Set<Int> = []
    private var spokenNear: Set<Int> = []

    static func farM(speedMps: Double) -> Double {
        min(1_000, max(250, 30 * usable(speedMps)))
    }

    static func nearM(speedMps: Double) -> Double {
        min(150, max(40, 5 * usable(speedMps)))
    }

    mutating func reset() {
        planID = nil
        spokenFar = []
        spokenNear = []
    }

    mutating func prompts(plan: RoutePlan, position: RoutePosition, speedMps: Double) -> [Prompt] {
        if planID != plan.id {
            reset()
            planID = plan.id
        }
        guard let step = position.nextStep, let distance = position.toNextM else {
            return []
        }
        let near = Self.nearM(speedMps: speedMps)
        if distance <= near, !spokenNear.contains(step) {
            spokenNear.insert(step)
            spokenFar.insert(step)
            return [Prompt(step: step, stage: .near, toNextM: distance)]
        }
        if distance > near + Self.nearGapM, distance <= Self.farM(speedMps: speedMps),
            !spokenFar.contains(step)
        {
            spokenFar.insert(step)
            return [Prompt(step: step, stage: .far, toNextM: distance)]
        }
        return []
    }

    private static func usable(_ speed: Double) -> Double {
        speed.isFinite && speed > 0 ? speed : 0
    }
}

nonisolated enum PromptText {
    static let arrived = "Arrived"
    static let rerouting = "Rerouting"
    static let fallback = "Continue"

    static func text(_ prompt: Prompt, plan: RoutePlan, units: UnitSystem) -> String {
        let instruction = instruction(plan, step: prompt.step)
        switch prompt.stage {
        case .far: return "In \(DistanceText.spoken(prompt.toNextM, units)), \(instruction)"
        case .near: return instruction
        }
    }

    static func retarget(distanceM: Double?, units: UnitSystem) -> String {
        guard let distanceM, distanceM.isFinite else {
            return "New target"
        }
        return "New target, \(DistanceText.spoken(distanceM, units))"
    }

    static func instruction(_ plan: RoutePlan, step: Int) -> String {
        guard plan.steps.indices.contains(step) else {
            return fallback
        }
        let text = plan.steps[step].instruction.trimmingCharacters(in: .whitespacesAndNewlines)
        return text.isEmpty ? fallback : text
    }
}
