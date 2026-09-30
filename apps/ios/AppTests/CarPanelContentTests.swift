import CarPlay
import SdrmmCore
import XCTest

@testable import SDRmm

final class CarPanelContentTests: XCTestCase {
    func testLiveBearingDetail() throws {
        let content = CarPanelContent.make(
            df: Fixtures.df(),
            pose: FakeScenarios.pose(heading: 90),
            here: Fixtures.origin,
            units: .metric
        )
        XCTAssertEqual(content.bearing, "047\u{00B0}")
        XCTAssertEqual(content.bearingDetail, "Bearing 62%")
        XCTAssertEqual(content.infoBearing, "047\u{00B0} 62%")
        XCTAssertEqual(content.guidance, "215\u{00B0}")
        XCTAssertEqual(content.guidanceDetail, "Cross 1.2 km")
        XCTAssertTrue(content.canNavigate)
        XCTAssertEqual(try XCTUnwrap(content.targetDistanceM), 1_200, accuracy: 1)
    }

    func testNoHeadingState() {
        let content = CarPanelContent.make(
            df: Fixtures.df(state: .noHeading),
            pose: FakeScenarios.pose(heading: nil),
            here: nil,
            units: .metric
        )
        XCTAssertEqual(content.bearing, "-")
        XCTAssertEqual(content.bearingDetail, "No heading")
        XCTAssertNil(content.confidence)
        XCTAssertNil(content.targetDistanceM)
    }

    func testNoTargetDisablesNavigate() {
        let content = CarPanelContent.make(
            df: Fixtures.df(guidance: .some(nil), target: .some(nil)),
            pose: nil,
            here: Fixtures.origin,
            units: .metric
        )
        XCTAssertFalse(content.canNavigate)
        XCTAssertNil(content.targetDistanceM)
        XCTAssertEqual(content.guidanceDetail, "No guidance")
        XCTAssertEqual(content.guidance, "-")
    }

    func testNothingYetIsWaiting() {
        let content = CarPanelContent.make(df: nil, pose: nil, here: nil, units: .imperial)
        XCTAssertEqual(content.bearingDetail, "Waiting")
        XCTAssertFalse(content.canNavigate)
    }

    @MainActor func testOnlyTheMissionsControlsAreOffered() {
        let full = CarPanelContent.make(
            df: Fixtures.df(),
            pose: nil,
            here: nil,
            units: .metric,
            controls: [.calibrate, .clearFusion, .targetMode]
        )
        XCTAssertTrue(full.canCalibrate)
        XCTAssertTrue(full.canClear)
        let fusion = CarPanelContent.make(
            df: Fixtures.df(),
            pose: nil,
            here: nil,
            units: .metric,
            controls: [.targetMode]
        )
        XCTAssertFalse(fusion.canCalibrate)
        XCTAssertFalse(fusion.canClear)
        let info = CarDfInfo(onNavigate: {}, onCalibrate: {}, onClear: {})
        XCTAssertEqual(info.template(full).actions.map(\.title), ["Calibrate", "Clear", "Navigate"])
        XCTAssertEqual(info.template(fusion).actions.map(\.title), ["Navigate"])
    }

    func testEstimateKindReadsApproach() {
        let content = CarPanelContent.make(
            df: Fixtures.df(guidance: Fixtures.guidance(.estimate, distance: 300)),
            pose: nil,
            here: nil,
            units: .metric
        )
        XCTAssertEqual(content.guidanceDetail, "Approach 300 m")
    }
}
