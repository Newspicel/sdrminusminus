nonisolated enum ManeuverKind: Equatable, Sendable, CaseIterable {
    case depart, straight, slightLeft, slightRight, left, right, sharpLeft, sharpRight, uTurnLeft, uTurnRight
    case arrive

    var symbolName: String {
        switch self {
        case .depart, .straight: "arrow.up"
        case .slightLeft: "arrow.up.left"
        case .slightRight: "arrow.up.right"
        case .left: "arrow.turn.up.left"
        case .right: "arrow.turn.up.right"
        case .sharpLeft: "arrow.turn.left.down"
        case .sharpRight: "arrow.turn.right.down"
        case .uTurnLeft, .uTurnRight: "arrow.uturn.down"
        case .arrive: "flag.checkered"
        }
    }

    static func classify(turnDeg: Double) -> ManeuverKind {
        let magnitude = abs(turnDeg)
        let left = turnDeg < 0
        switch magnitude {
        case ...15: return .straight
        case ...45: return left ? .slightLeft : .slightRight
        case ...135: return left ? .left : .right
        case ...170: return left ? .sharpLeft : .sharpRight
        default: return left ? .uTurnLeft : .uTurnRight
        }
    }
}
