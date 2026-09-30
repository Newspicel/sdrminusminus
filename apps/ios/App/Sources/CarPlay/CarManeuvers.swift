import CarPlay
import Foundation

nonisolated struct CarManeuverSpec: Equatable, Sendable {
    let step: Int
    let instruction: String
    let symbolName: String
    let kind: ManeuverKind
    let distanceM: Double
}

nonisolated enum CarManeuverStateKind: Equatable, Sendable {
    case initial, continuing, prepare, execute
}

nonisolated enum CarManeuvers {
    static let executeM = 60.0
    static let prepareM = 400.0
    static let initialS = 5.0
    static let upcomingCount = 2

    static func specs(for plan: RoutePlan) -> [CarManeuverSpec] {
        plan.steps.indices.compactMap { index in
            let step = plan.steps[index]
            let blank = step.instruction.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
            if index == 0, blank {
                return nil
            }
            let approach = index == 0 ? 0 : step.startM - plan.steps[index - 1].startM
            return CarManeuverSpec(
                step: index,
                instruction: PromptText.instruction(plan, step: index),
                symbolName: step.maneuver.symbolName,
                kind: step.maneuver,
                distanceM: max(0, approach)
            )
        }
    }

    static func upcoming(plan: RoutePlan, position: RoutePosition) -> [CarManeuverSpec] {
        guard let next = position.nextStep else {
            return []
        }
        return Array(specs(for: plan).filter { $0.step >= next }.prefix(upcomingCount))
    }

    static func state(toNextM: Double, stepAgeS: Double) -> CarManeuverStateKind {
        if toNextM <= executeM {
            return .execute
        }
        if toNextM <= prepareM {
            return .prepare
        }
        return stepAgeS < initialS ? .initial : .continuing
    }
}

nonisolated extension CarManeuverStateKind {
    var carPlayState: CPManeuverState {
        switch self {
        case .initial: .initial
        case .continuing: .continue
        case .prepare: .prepare
        case .execute: .execute
        }
    }
}

nonisolated extension ManeuverKind {
    var carPlayType: CPManeuverType {
        switch self {
        case .depart: .startRoute
        case .straight: .straightAhead
        case .slightLeft: .slightLeftTurn
        case .slightRight: .slightRightTurn
        case .left: .leftTurn
        case .right: .rightTurn
        case .sharpLeft: .sharpLeftTurn
        case .sharpRight: .sharpRightTurn
        case .uTurnLeft, .uTurnRight: .uTurn
        case .arrive: .arriveAtDestination
        }
    }
}
