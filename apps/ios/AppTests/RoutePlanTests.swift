import SdrmmCore
import XCTest

@testable import SDRmm

final class RoutePlanTests: XCTestCase {
    func testJunctionPointNotDuplicated() {
        let plan = Fixtures.lPlan()
        XCTAssertEqual(plan.points.count, 3)
        XCTAssertEqual(plan.steps.map(\.startIndex), [0, 0, 1, 2])
    }

    func testCumulativeDistances() throws {
        let plan = Fixtures.lPlan()
        XCTAssertEqual(plan.cumulativeM.count, plan.points.count)
        XCTAssertEqual(plan.cumulativeM.first, 0)
        XCTAssertEqual(plan.cumulativeM[1], 600, accuracy: 1)
        XCTAssertEqual(plan.distanceM, 1_400, accuracy: 1)
        XCTAssertEqual(try XCTUnwrap(plan.steps.last).startM, plan.distanceM, accuracy: 1e-9)
        XCTAssertEqual(plan.steps[2].startM, 600, accuracy: 1)
        XCTAssertEqual(plan.travelTimeS, 240)
    }

    func testRightAngleTurnIsRight() {
        XCTAssertEqual(Fixtures.lPlan().steps[2].maneuver, .right)
        let left = RoutePlanBuilder.plan(
            name: "Left",
            travelTimeS: 60,
            steps: [
                RawStep(instruction: "Go", notice: nil, distanceM: 300, points: [Fixtures.origin, corner]),
                RawStep(instruction: "Turn left", notice: nil, distanceM: 300, points: [corner, west]),
                RawStep(instruction: "Arrive", notice: nil, distanceM: 0, points: [west]),
            ]
        )
        XCTAssertEqual(left.steps[1].maneuver, .left)
    }

    func testFirstDepartLastArrive() {
        let plan = Fixtures.steppedPlan()
        XCTAssertEqual(plan.steps.first?.maneuver, .depart)
        XCTAssertEqual(plan.steps.last?.maneuver, .arrive)
        XCTAssertEqual(plan.steps[2].maneuver, .straight)
    }

    func testWrap180() {
        XCTAssertEqual(RoutePlanBuilder.wrap180(190), -170)
        XCTAssertEqual(RoutePlanBuilder.wrap180(-190), 170)
        XCTAssertEqual(RoutePlanBuilder.wrap180(90), 90)
    }

    private var corner: LatLon { Fixtures.offset(Fixtures.origin, 0, 300) }
    private var west: LatLon { Fixtures.offset(corner, 270, 300) }
}
