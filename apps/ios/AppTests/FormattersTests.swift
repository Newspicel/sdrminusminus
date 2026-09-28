import XCTest

@testable import SDRmm

final class FormattersTests: XCTestCase {
    func testDistanceShortMetric() {
        XCTAssertEqual(DistanceText.short(43, .metric), "45 m")
        XCTAssertEqual(DistanceText.short(318, .metric), "320 m")
        XCTAssertEqual(DistanceText.short(1_234, .metric), "1.2 km")
        XCTAssertEqual(DistanceText.short(14_200, .metric), "14 km")
        XCTAssertEqual(DistanceText.short(.nan, .metric), "-")
        XCTAssertEqual(DistanceText.short(.infinity, .metric), "-")
    }

    func testDistanceShortImperial() {
        XCTAssertEqual(DistanceText.short(137, .imperial), "450 ft")
        XCTAssertEqual(DistanceText.short(1_931, .imperial), "1.2 mi")
        XCTAssertEqual(DistanceText.short(22_531, .imperial), "14 mi")
        XCTAssertEqual(DistanceText.short(.nan, .imperial), "-")
    }

    func testDistanceSpoken() {
        XCTAssertEqual(DistanceText.spoken(318, .metric), "320 meters")
        XCTAssertEqual(DistanceText.spoken(1_234, .metric), "1.2 kilometers")
        XCTAssertEqual(DistanceText.spoken(1_000, .metric), "1 kilometer")
        XCTAssertEqual(DistanceText.spoken(137, .imperial), "450 feet")
        XCTAssertEqual(DistanceText.spoken(1_931, .imperial), "1.2 miles")
        XCTAssertEqual(DistanceText.spoken(1_609.344, .imperial), "1 mile")
        XCTAssertEqual(DistanceText.spoken(.nan, .metric), "-")
    }

    func testFrequency() {
        XCTAssertEqual(FrequencyText.text(1_296_000_000), "1.2960 GHz")
        XCTAssertEqual(FrequencyText.text(145_500_000), "145.500 MHz")
        XCTAssertEqual(FrequencyText.text(12_500), "12.5 kHz")
        XCTAssertEqual(FrequencyText.text(440), "440 Hz")
        XCTAssertEqual(FrequencyText.text(.nan), "-")
    }

    func testAngle() {
        XCTAssertEqual(AngleText.degrees(7), "007\u{00B0}")
        XCTAssertEqual(AngleText.degrees(137.4), "137\u{00B0}")
        XCTAssertEqual(AngleText.degrees(359.6), "000\u{00B0}")
        XCTAssertEqual(AngleText.degrees(-10), "350\u{00B0}")
        XCTAssertEqual(AngleText.degrees(720), "000\u{00B0}")
        XCTAssertEqual(AngleText.degrees(-0.2), "000\u{00B0}")
        XCTAssertEqual(AngleText.degrees(-359.6), "000\u{00B0}")
        XCTAssertEqual(AngleText.degrees(3.6e12 + 7), "007\u{00B0}")
        XCTAssertEqual(AngleText.degrees(1e300).count, 4)
        XCTAssertEqual(AngleText.degrees(.nan), "-")
    }

    func testDuration() {
        XCTAssertEqual(DurationText.text(30), "<1 min")
        XCTAssertEqual(DurationText.text(720), "12 min")
        XCTAssertEqual(DurationText.text(3_900), "1 h 05")
        XCTAssertEqual(DurationText.text(3_599), "1 h 00")
        XCTAssertEqual(DurationText.text(.infinity), "-")
        XCTAssertEqual(DurationText.text(1e300), "-")
    }

    func testLevel() {
        XCTAssertEqual(LevelText.db(nil), "-")
        XCTAssertEqual(LevelText.db(.nan), "-")
        XCTAssertEqual(LevelText.db(-62.34), "-62.3 dB")
    }

    func testTuneInput() {
        XCTAssertEqual(TuneInput.hertz("145,5"), 145_500_000)
        XCTAssertEqual(TuneInput.hertz(" 433.92 "), 433_920_000)
        XCTAssertNil(TuneInput.hertz("abc"))
        XCTAssertNil(TuneInput.hertz("0"))
        XCTAssertNil(TuneInput.hertz("100000"))
    }
}
