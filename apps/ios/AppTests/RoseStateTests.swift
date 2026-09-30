import SdrmmCore
import XCTest

@testable import SDRmm

final class RoseStateTests: XCTestCase {
    func testHeadingUpUsesRelative() throws {
        let rose = RoseState.make(view: Fixtures.df(heading: 90), pose: FakeScenarios.pose(heading: 90))
        XCTAssertTrue(rose.headingUp)
        XCTAssertEqual(try XCTUnwrap(rose.bearingDeg), 47, accuracy: 1e-6)
        XCTAssertEqual(try XCTUnwrap(rose.guidanceDeg), 125, accuracy: 1e-6)
        XCTAssertEqual(rose.sigmaDeg, 6)
    }

    func testNoHeadingIsNorthUp() throws {
        let rose = RoseState.make(view: Fixtures.df(heading: nil), pose: FakeScenarios.pose(heading: nil))
        XCTAssertFalse(rose.headingUp)
        XCTAssertEqual(rose.northDeg, 0)
        XCTAssertEqual(try XCTUnwrap(rose.bearingDeg), 137, accuracy: 1e-6)
        XCTAssertEqual(try XCTUnwrap(rose.guidanceDeg), 215, accuracy: 1e-6)
        XCTAssertEqual(rose.sideText, "North up")
    }

    func testRelativeBearingWithoutPoseStaysNorthUp() throws {
        let rose = RoseState.make(view: Fixtures.df(heading: 90), pose: nil)
        XCTAssertFalse(rose.headingUp)
        XCTAssertEqual(try XCTUnwrap(rose.bearingDeg), 137, accuracy: 1e-6)
    }

    func testNotLiveHidesBearing() {
        for state in [DfState.waiting, .calibrating, .phaseUnknown, .noHeading, .squelched] {
            let rose = RoseState.make(view: Fixtures.df(state: state), pose: FakeScenarios.pose(heading: 90))
            XCTAssertNil(rose.bearingDeg, "\(state)")
            XCTAssertNil(rose.sigmaDeg)
            XCTAssertNil(rose.sideText)
        }
    }

    func testNorthMarker() {
        let rose = RoseState.make(view: Fixtures.df(heading: 90), pose: FakeScenarios.pose(heading: 90))
        XCTAssertEqual(rose.northDeg, 270, accuracy: 1e-9)
    }

    func testSideText() {
        var rose = RoseState(bearingDeg: 40, sigmaDeg: nil, guidanceDeg: nil, northDeg: 0, headingUp: true)
        XCTAssertEqual(rose.sideText, "40\u{00B0} right")
        rose.bearingDeg = 320
        XCTAssertEqual(rose.sideText, "40\u{00B0} left")
        rose.bearingDeg = 0.2
        XCTAssertEqual(rose.sideText, "Ahead")
    }
}
