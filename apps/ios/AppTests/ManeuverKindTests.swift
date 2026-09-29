import CarPlay
import UIKit
import XCTest

@testable import SDRmm

final class ManeuverKindTests: XCTestCase {
    func testClassifyThresholds() {
        XCTAssertEqual(ManeuverKind.classify(turnDeg: 0), .straight)
        XCTAssertEqual(ManeuverKind.classify(turnDeg: 15), .straight)
        XCTAssertEqual(ManeuverKind.classify(turnDeg: -20), .slightLeft)
        XCTAssertEqual(ManeuverKind.classify(turnDeg: 45), .slightRight)
        XCTAssertEqual(ManeuverKind.classify(turnDeg: 90), .right)
        XCTAssertEqual(ManeuverKind.classify(turnDeg: -135), .left)
        XCTAssertEqual(ManeuverKind.classify(turnDeg: -150), .sharpLeft)
        XCTAssertEqual(ManeuverKind.classify(turnDeg: 175), .uTurnRight)
        XCTAssertEqual(ManeuverKind.classify(turnDeg: -179), .uTurnLeft)
    }

    func testEverySymbolExists() {
        for kind in ManeuverKind.allCases {
            XCTAssertNotNil(UIImage(systemName: kind.symbolName), "\(kind)")
        }
    }

    func testCarPlayMapping() {
        let expected: [ManeuverKind: CPManeuverType] = [
            .depart: .startRoute,
            .straight: .straightAhead,
            .slightLeft: .slightLeftTurn,
            .slightRight: .slightRightTurn,
            .left: .leftTurn,
            .right: .rightTurn,
            .sharpLeft: .sharpLeftTurn,
            .sharpRight: .sharpRightTurn,
            .uTurnLeft: .uTurn,
            .uTurnRight: .uTurn,
            .arrive: .arriveAtDestination,
        ]
        XCTAssertEqual(expected.count, ManeuverKind.allCases.count)
        for kind in ManeuverKind.allCases {
            XCTAssertEqual(kind.carPlayType, expected[kind], "\(kind)")
        }
    }
}
