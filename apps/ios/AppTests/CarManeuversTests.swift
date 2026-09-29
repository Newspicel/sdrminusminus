import CarPlay
import XCTest

@testable import SDRmm

final class CarManeuversTests: XCTestCase {
    func testSpecsSkipEmptyDepart() {
        let plan = Fixtures.steppedPlan()
        let specs = CarManeuvers.specs(for: plan)
        XCTAssertEqual(specs.first?.step, 1)
        XCTAssertEqual(specs.count, plan.steps.count - 1)
        XCTAssertEqual(specs.last?.kind, .arrive)
        XCTAssertEqual(specs.last?.symbolName, "flag.checkered")
        let named = Fixtures.straightPlan()
        XCTAssertEqual(CarManeuvers.specs(for: named).first?.step, 0)
    }

    func testSpecDistanceIsTheApproach() throws {
        let plan = Fixtures.lPlan()
        let turn = try XCTUnwrap(CarManeuvers.specs(for: plan).first { $0.step == 2 })
        XCTAssertEqual(turn.distanceM, 600, accuracy: 1)
        XCTAssertEqual(turn.instruction, "Turn right")
        XCTAssertEqual(turn.kind, .right)
    }

    func testUpcomingIsNextTwo() {
        let plan = Fixtures.steppedPlan()
        let upcoming = CarManeuvers.upcoming(plan: plan, position: Fixtures.position(next: 3, toNextM: 50))
        XCTAssertEqual(upcoming.map(\.step), [3, 4])
        let last = CarManeuvers.upcoming(plan: plan, position: Fixtures.position(next: 6, toNextM: 50))
        XCTAssertEqual(last.map(\.step), [6])
        XCTAssertTrue(
            CarManeuvers.upcoming(plan: plan, position: Fixtures.position(next: nil, toNextM: nil)).isEmpty
        )
    }

    func testStateThresholds() {
        XCTAssertEqual(CarManeuvers.state(toNextM: 60, stepAgeS: 10), .execute)
        XCTAssertEqual(CarManeuvers.state(toNextM: 61, stepAgeS: 10), .prepare)
        XCTAssertEqual(CarManeuvers.state(toNextM: 400, stepAgeS: 1), .prepare)
        XCTAssertEqual(CarManeuvers.state(toNextM: 401, stepAgeS: 10), .continuing)
        XCTAssertEqual(CarManeuvers.state(toNextM: 1_000, stepAgeS: 2), .initial)
    }

    func testStateMapsToCarPlay() {
        XCTAssertEqual(CarManeuverStateKind.initial.carPlayState, .initial)
        XCTAssertEqual(CarManeuverStateKind.continuing.carPlayState, .continue)
        XCTAssertEqual(CarManeuverStateKind.prepare.carPlayState, .prepare)
        XCTAssertEqual(CarManeuverStateKind.execute.carPlayState, .execute)
    }
}
