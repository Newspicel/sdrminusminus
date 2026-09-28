import XCTest

@testable import SDRmm

@MainActor
final class AppLaunchTests: XCTestCase {
    func testHostUsesFakeCore() {
        XCTAssertTrue(AppRuntime.isUnitTestHost)
        XCTAssertNil(AppRuntime.coreFailure)
        let fake = AppRuntime.model.core as? FakeCore
        XCTAssertEqual(fake?.scenario, .fresh)
    }

    func testHostStartsOnPair() {
        XCTAssertTrue(AppRuntime.model.needsPairing)
    }
}
