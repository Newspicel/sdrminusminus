import SdrmmCore
import XCTest

@testable import SDRmm

final class RouteTrackTests: XCTestCase {
    private let origin = Fixtures.origin

    func testStraightLineProjection() {
        var track = RouteTrack(plan: Fixtures.straightPlan())
        let beside = Fixtures.offset(Fixtures.offset(origin, 0, 500), 90, 10)
        let position = track.locate(beside)
        XCTAssertEqual(position.alongM, 500, accuracy: 1)
        XCTAssertEqual(position.offRouteM, 10, accuracy: 0.5)
        XCTAssertEqual(position.remainingM, 500, accuracy: 1)
        XCTAssertEqual(position.nextStep, 1)
    }

    func testNextStepNeverBehind() throws {
        let plan = Fixtures.steppedPlan()
        var track = RouteTrack(plan: plan)
        let stepTwo = plan.steps[2].startM
        let position = track.locate(Fixtures.offset(origin, 0, stepTwo + 6))
        XCTAssertEqual(position.nextStep, 3)
        XCTAssertEqual(
            try XCTUnwrap(position.toNextM),
            plan.steps[3].startM - position.alongM,
            accuracy: 1e-9
        )
        let before = track.locate(Fixtures.offset(origin, 0, stepTwo - 20))
        XCTAssertEqual(before.nextStep, 2)
    }

    func testOffRouteDistance() {
        var track = RouteTrack(plan: Fixtures.straightPlan())
        let away = Fixtures.offset(Fixtures.offset(origin, 0, 300), 90, 200)
        XCTAssertEqual(track.locate(away).offRouteM, 200, accuracy: 2)
    }

    func testWindowRecoversAfterJump() {
        let plan = Fixtures.steppedPlan(stepEveryM: 400, steps: 5, pointEveryM: 20)
        var track = RouteTrack(plan: plan)
        _ = track.locate(origin)
        XCTAssertEqual(track.segment, 0)
        let far = track.locate(Fixtures.offset(Fixtures.offset(origin, 0, 1_610), 90, 5))
        XCTAssertEqual(far.alongM, 1_610, accuracy: 1)
        XCTAssertEqual(far.offRouteM, 5, accuracy: 0.5)
        XCTAssertGreaterThan(track.segment, 42)
    }

    func testRemainingZeroAtEnd() {
        var track = RouteTrack(plan: Fixtures.straightPlan())
        let past = track.locate(Fixtures.offset(origin, 0, 1_050))
        XCTAssertEqual(past.remainingM, 0, accuracy: 1e-9)
        XCTAssertNil(past.nextStep)
        XCTAssertNil(past.toNextM)
    }

    func testSinglePointPlan() {
        let plan = RoutePlanBuilder.plan(
            name: "Point",
            travelTimeS: 0,
            steps: [RawStep(instruction: "Arrive", notice: nil, distanceM: 0, points: [origin])]
        )
        var track = RouteTrack(plan: plan)
        let position = track.locate(Fixtures.offset(origin, 90, 100))
        XCTAssertEqual(position.offRouteM, 100, accuracy: 0.5)
        XCTAssertEqual(position.alongM, 0)
        XCTAssertEqual(position.remainingM, 0)
        XCTAssertNil(position.nextStep)
    }
}
