import Foundation
import SdrmmCore
import XCTest

@testable import SDRmm

final class RadarImageRendererTests: XCTestCase {
    private func image(width: UInt32, height: UInt32, bytes: Int? = nil) -> RgbaImage {
        let count = bytes ?? Int(width * height * 4)
        return RgbaImage(
            width: width,
            height: height,
            rgba: Data(repeating: 0x80, count: count),
            rangeMaxKm: 60,
            dopplerSpanHz: 400
        )
    }

    func testValidImageSize() throws {
        let rendered = try RadarImageRenderer.cgImage(image(width: 4, height: 2))
        XCTAssertEqual(rendered.width, 4)
        XCTAssertEqual(rendered.height, 2)
        XCTAssertEqual(rendered.bitsPerPixel, 32)
        XCTAssertEqual(rendered.bytesPerRow, 16)
        XCTAssertEqual(rendered.alphaInfo, .premultipliedLast)
    }

    func testBadSizeThrows() {
        XCTAssertThrowsError(try RadarImageRenderer.cgImage(image(width: 4, height: 2, bytes: 31))) { error in
            XCTAssertEqual(error as? RadarImageError, .badSize(expected: 32, actual: 31))
        }
    }

    func testEmptyThrows() {
        XCTAssertThrowsError(try RadarImageRenderer.cgImage(image(width: 0, height: 2))) { error in
            XCTAssertEqual(error as? RadarImageError, .emptyImage)
        }
        XCTAssertThrowsError(try RadarImageRenderer.cgImage(image(width: 3, height: 0))) { error in
            XCTAssertEqual(error as? RadarImageError, .emptyImage)
        }
    }

    @MainActor
    func testModelKeepsAxesAndFlagsFailures() {
        let model = RadarModel()
        model.apply(image: image(width: 4, height: 2))
        XCTAssertNotNil(model.image)
        XCTAssertFalse(model.imageFailed)
        XCTAssertEqual(model.axes?.rangeEnd, "60 km")
        XCTAssertEqual(model.axes?.dopplerTop, "+200 Hz")
        XCTAssertEqual(model.axes?.dopplerBottom, "-200 Hz")
        model.apply(image: image(width: 4, height: 2, bytes: 3))
        XCTAssertNil(model.image)
        XCTAssertTrue(model.imageFailed)
        XCTAssertEqual(RadarAxes(rangeMaxKm: .nan, dopplerSpanHz: .infinity).rangeEnd, "-")
        model.apply(image: image(width: 4, height: 2))
        XCTAssertFalse(model.imageFailed)
    }

    private static func track(_ index: Int) -> RadarTrack {
        track(index, snr: Float(index))
    }

    private static func track(_ index: Int, snr: Float) -> RadarTrack {
        let even = index.isMultiple(of: 2)
        let doppler: Float = even ? -35 : 12.4
        let bearing: Float? = index == 10 ? 137 : nil
        return RadarTrack(
            id: UInt32(index),
            rangeKm: Float(index) * 1.25,
            dopplerHz: doppler,
            speedMps: 10,
            snrDb: snr,
            closing: even,
            coasting: index == 9,
            bearingDeg: bearing
        )
    }

    @MainActor
    func testRowsSortedBySnrTopEight() {
        let model = RadarModel()
        let tracks = [Self.track(11, snr: .nan)] + (1...10).map { Self.track($0) }
        model.apply(RadarView(mission: "r", echoes: 10, tracks: tracks, stale: false, problems: []))
        let rows = model.rows
        XCTAssertEqual(rows.map(\.id), [10, 9, 8, 7, 6, 5, 4, 3])
        XCTAssertEqual(
            rows[0],
            RadarRow(
                id: 10,
                name: "T10",
                range: "12.5 km",
                doppler: "-35 Hz",
                motion: "Closing",
                bearing: "137\u{00B0}",
                closing: true,
                coasting: false
            )
        )
        XCTAssertEqual(rows[1].doppler, "+12 Hz")
        XCTAssertEqual(rows[1].motion, "Opening")
        XCTAssertTrue(rows[1].coasting)
        XCTAssertNil(rows[1].bearing)
        XCTAssertEqual(RadarText.name(7), "T07")
        XCTAssertEqual(RadarText.echoes(6), "6 echoes")
        XCTAssertEqual(RadarText.echoes(1), "1 echo")
    }

    @MainActor
    func testMissionResetClearsEverything() {
        let model = RadarModel()
        model.apply(FakeScenarios.radar())
        model.apply(image: FakeScenarios.radarImage())
        model.missionClosed()
        XCTAssertNil(model.view)
        XCTAssertNil(model.image)
        XCTAssertNil(model.axes)
        XCTAssertTrue(model.rows.isEmpty)
    }
}
