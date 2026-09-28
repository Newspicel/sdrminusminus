import CoreLocation
import CoreMotion
import SdrmmCore
import XCTest

@testable import SDRmm

final class SampleMappingTests: XCTestCase {
    private func location(horizontal: Double = 5, invalid: Bool) -> CLLocation {
        CLLocation(
            coordinate: CLLocationCoordinate2D(latitude: 52.52, longitude: 13.405),
            altitude: 40,
            horizontalAccuracy: horizontal,
            verticalAccuracy: invalid ? -1 : 3,
            course: invalid ? -1 : 90,
            courseAccuracy: invalid ? -1 : 5,
            speed: invalid ? -1 : 10,
            speedAccuracy: invalid ? -1 : 1,
            timestamp: Date(timeIntervalSince1970: 1_000.25)
        )
    }

    func testNegativeCourseAndSpeedBecomeNil() throws {
        let sample = try XCTUnwrap(SampleMapping.location(location(invalid: true)))
        XCTAssertNil(sample.speedMps)
        XCTAssertNil(sample.speedAccMps)
        XCTAssertNil(sample.courseDeg)
        XCTAssertNil(sample.courseAccDeg)
        XCTAssertNil(sample.vAccM)
        XCTAssertNil(sample.altM)
        XCTAssertEqual(sample.hAccM, 5)
    }

    func testValidFieldsAreKept() throws {
        let sample = try XCTUnwrap(SampleMapping.location(location(invalid: false)))
        XCTAssertEqual(sample.tUnixMs, 1_000_250)
        XCTAssertEqual(sample.lat, 52.52)
        XCTAssertEqual(sample.lon, 13.405)
        XCTAssertEqual(sample.altM, 40)
        XCTAssertEqual(sample.vAccM, 3)
        XCTAssertEqual(sample.speedMps, 10)
        XCTAssertEqual(sample.speedAccMps, 1)
        XCTAssertEqual(sample.courseDeg, 90)
        XCTAssertEqual(sample.courseAccDeg, 5)
    }

    func testNegativeAccuracyDropsSample() {
        XCTAssertNil(SampleMapping.location(location(horizontal: -1, invalid: false)))
    }

    func testUptimeToUnixMillis() {
        let wall = Date(timeIntervalSince1970: 1_000)
        XCTAssertEqual(SampleMapping.unixMillis(uptime: 100, uptimeNow: 102.5, wallNow: wall), 997_500)
        XCTAssertEqual(SampleMapping.unixMillis(uptime: 50.0004, uptimeNow: 50, wallNow: wall), 1_000_000)
    }

    func testInvalidHeadingAccuracyIsNil() {
        let at = Date(timeIntervalSince1970: 2_000)
        let invalid = SampleMapping.heading(trueDeg: -1, magneticDeg: 12, accuracyDeg: -1, at: at)
        XCTAssertNil(invalid.trueDeg)
        XCTAssertNil(invalid.accuracyDeg)
        XCTAssertEqual(invalid.magneticDeg, 12)
        XCTAssertEqual(invalid.tUnixMs, 2_000_000)
        let valid = SampleMapping.heading(trueDeg: 14, magneticDeg: 12, accuracyDeg: 7, at: at)
        XCTAssertEqual(valid.trueDeg, 14)
        XCTAssertEqual(valid.accuracyDeg, 7)
    }

    func testMagneticAccuracyMapping() {
        XCTAssertEqual(SampleMapping.magAccuracy(.uncalibrated), .uncalibrated)
        XCTAssertEqual(SampleMapping.magAccuracy(.low), .low)
        XCTAssertEqual(SampleMapping.magAccuracy(.medium), .medium)
        XCTAssertEqual(SampleMapping.magAccuracy(.high), .high)
    }
}
