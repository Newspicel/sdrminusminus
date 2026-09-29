import SdrmmCore
import XCTest

@testable import SDRmm

final class CarMapSceneTests: XCTestCase {
    func testLayersFilterContent() {
        let df = Fixtures.df()
        let all = CarMapScene.make(df: df, plan: nil, layers: MapLayers(), follow: .user)
        XCTAssertEqual(all.rays, df.overlay.rays)
        XCTAssertEqual(all.heat, df.overlay.heat)
        XCTAssertEqual(all.ellipse, df.overlay.ellipse)
        let none = CarMapScene.make(
            df: df,
            plan: nil,
            layers: MapLayers(rays: false, heat: false, ellipse: false),
            follow: .user
        )
        XCTAssertTrue(none.rays.isEmpty)
        XCTAssertTrue(none.heat.isEmpty)
        XCTAssertTrue(none.ellipse.isEmpty)
        XCTAssertEqual(none.stations, df.overlay.stations)
        XCTAssertEqual(none.target, df.target)
        XCTAssertEqual(none.estimate, df.estimate?.at)
    }

    func testEqualScenesAreEqual() {
        let df = Fixtures.df()
        let plan = Fixtures.lPlan()
        let first = CarMapScene.make(df: df, plan: plan, layers: MapLayers(), follow: .userHeading)
        let second = CarMapScene.make(df: df, plan: plan, layers: MapLayers(), follow: .userHeading)
        XCTAssertEqual(first, second)
        XCTAssertNotEqual(first, CarMapScene.make(df: df, plan: plan, layers: MapLayers(), follow: .free))
        XCTAssertEqual(CarMapScene.make(df: nil, plan: nil, layers: MapLayers(), follow: .user), .empty)
    }

    func testRouteFromPlan() {
        let plan = Fixtures.lPlan()
        let scene = CarMapScene.make(df: nil, plan: plan, layers: MapLayers(), follow: .user)
        XCTAssertEqual(scene.route, plan.points)
        XCTAssertTrue(scene.rays.isEmpty)
    }

    func testBearingRayFollowsTrueBearing() throws {
        let here = Fixtures.origin
        let ray = try XCTUnwrap(CarBearingRay.make(df: Fixtures.df(), here: here))
        XCTAssertEqual(ray.bearingDeg, 137, accuracy: 1e-6)
        XCTAssertEqual(geoBearingDeg(from: here, to: ray.end), 137, accuracy: 0.1)
        XCTAssertEqual(geoDistanceM(from: here, to: ray.end), ray.lengthM, accuracy: 1)
        XCTAssertNil(CarBearingRay.make(df: Fixtures.df(state: .calibrating), here: here))
        XCTAssertNil(CarBearingRay.make(df: Fixtures.df(), here: nil))
    }
}
