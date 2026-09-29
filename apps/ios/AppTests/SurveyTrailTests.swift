import MapKit
import SdrmmCore
import SwiftUI
import XCTest

@testable import SDRmm

@MainActor
struct SurveyFixture {
    let core = FakeCore(scenario: .survey, ticking: false)
    let sink = ErrorSink()
    let clock = ManualClock()
    let model: SurveyModel

    init() {
        let sink = sink
        let clock = clock
        model = SurveyModel(core: core, report: { sink.report($0) }, now: { clock.now })
        model.missionOpened()
    }

    var sent: [MissionCommand] {
        core.calls.compactMap { call in
            if case .send(let command) = call {
                return command
            }
            return nil
        }
    }
}

final class SurveyTrailTests: XCTestCase {
    private static func point(_ index: Int, _ level: Float) -> SurveyPoint {
        SurveyPoint(at: LatLon(lat: 52.5, lon: 13.4 + Double(index) * 1e-4), levelDb: level)
    }

    private static func view(min: Float, max: Float, total: UInt64 = 10, recording: Bool = true) -> SurveyView
    {
        SurveyView(
            mission: FakeScenarios.surveyID,
            freqHz: 433_920_000,
            levelDb: -60,
            minDb: min,
            maxDb: max,
            total: total,
            recording: recording
        )
    }

    func testBinClamps() {
        XCTAssertEqual(SurveyTrail.bins, 8)
        XCTAssertEqual(SurveyTrail.bin(level: -100, min: -90, max: -10), 0)
        XCTAssertEqual(SurveyTrail.bin(level: -90, min: -90, max: -10), 0)
        XCTAssertEqual(SurveyTrail.bin(level: -50, min: -90, max: -10), 4)
        XCTAssertEqual(SurveyTrail.bin(level: -10.1, min: -90, max: -10), 7)
        XCTAssertEqual(SurveyTrail.bin(level: -10, min: -90, max: -10), 7)
        XCTAssertEqual(SurveyTrail.bin(level: 5, min: -90, max: -10), 7)
        XCTAssertEqual(SurveyTrail.bin(level: .nan, min: -90, max: -10), 0)
    }

    func testFlatRangeUsesOneDbWindow() {
        XCTAssertEqual(SurveyTrail.bin(level: -40, min: -40, max: -40), 4)
        XCTAssertEqual(SurveyTrail.bin(level: -70, min: -40, max: -39.5), 4)
        XCTAssertEqual(SurveyTrail.bin(level: -40, min: .nan, max: .nan), 4)
    }

    func testRunsShareBoundaryPoint() {
        let points = [
            Self.point(0, -90), Self.point(1, -88), Self.point(2, -20), Self.point(3, -15),
            Self.point(4, -89),
        ]
        let runs = SurveyTrail.runs(from: points, min: -90, max: -10)
        XCTAssertEqual(runs.map(\.bin), [0, 7, 0])
        XCTAssertEqual(runs.map(\.id), [0, 1, 2])
        XCTAssertEqual(runs[0].coordinates, [points[0].at, points[1].at, points[2].at])
        XCTAssertEqual(runs[1].coordinates, [points[2].at, points[3].at, points[4].at])
        XCTAssertEqual(runs[2].coordinates, [points[4].at])
    }

    func testExtendContinuesTheLastRun() {
        var runs = SurveyTrail.runs(from: [Self.point(0, -90)], min: -90, max: -10)
        var nextID = 1
        SurveyTrail.extend(
            &runs,
            with: [Self.point(1, -89), Self.point(2, -10)],
            min: -90,
            max: -10,
            nextID: &nextID
        )
        XCTAssertEqual(runs.map(\.bin), [0, 7])
        XCTAssertEqual(runs[0].coordinates.count, 3)
        XCTAssertEqual(nextID, 2)
    }

    @MainActor
    func testMaxRunsTrimsAndFlags() {
        let fixture = SurveyFixture()
        fixture.model.apply(Self.view(min: -90, max: -10))
        let points = (0...SurveyTrail.maxRuns).map { Self.point($0, $0.isMultiple(of: 2) ? -90 : -10) }
        fixture.model.append(points)
        XCTAssertEqual(fixture.model.runs.count, SurveyTrail.maxRuns)
        XCTAssertTrue(fixture.model.trimmed)
        XCTAssertEqual(fixture.model.runs.last?.coordinates, [points[points.count - 1].at])
    }

    @MainActor
    func testMaxPointsDropsOldest() {
        let fixture = SurveyFixture()
        fixture.model.apply(Self.view(min: -90, max: -10))
        fixture.model.append((0...SurveyTrail.maxPoints).map { Self.point($0, -50) })
        XCTAssertEqual(fixture.model.pointCount, SurveyTrail.keptPoints)
        XCTAssertTrue(fixture.model.trimmed)
        XCTAssertEqual(fixture.model.runs.count, 1)
        XCTAssertEqual(fixture.model.runs.first?.coordinates.count, SurveyTrail.keptPoints)
    }

    @MainActor
    func testRebuildOnRangeShift() {
        let fixture = SurveyFixture()
        fixture.model.apply(Self.view(min: -90, max: -10))
        fixture.model.append([Self.point(0, -60), Self.point(1, -55)])
        XCTAssertEqual(fixture.model.runs.map(\.bin), [3])
        fixture.clock.now = 0.5
        fixture.model.apply(Self.view(min: -88, max: -10))
        XCTAssertEqual(fixture.model.runs.map(\.bin), [3])
        fixture.model.apply(Self.view(min: -70, max: -50))
        XCTAssertEqual(fixture.model.runs.map(\.bin), [4, 6])
        fixture.clock.now = 1.5
        fixture.model.apply(Self.view(min: -60, max: -55))
        XCTAssertEqual(fixture.model.runs.map(\.bin), [4, 6])
        fixture.clock.now = 2.6
        fixture.model.apply(Self.view(min: -60, max: -55))
        XCTAssertEqual(fixture.model.runs.map(\.bin), [0, 7])
    }

    @MainActor
    func testPointsWithoutARangeAreDrawnOnceOneArrives() {
        let fixture = SurveyFixture()
        fixture.model.append([Self.point(0, .nan), Self.point(1, .nan)])
        XCTAssertTrue(fixture.model.runs.isEmpty)
        fixture.model.apply(Self.view(min: -90, max: -10))
        XCTAssertEqual(fixture.model.runs.count, 1)
        XCTAssertEqual(fixture.model.runs.first?.coordinates.count, 2)
        fixture.model.append([Self.point(2, -10)])
        XCTAssertEqual(fixture.model.runs.map(\.bin), [0, 7])
    }

    @MainActor
    func testServerClearEmptiesTheTrail() {
        let fixture = SurveyFixture()
        fixture.model.apply(Self.view(min: -90, max: -10))
        fixture.model.append([Self.point(0, -60), Self.point(1, -20)])
        fixture.model.apply(Self.view(min: 0, max: 0, total: 0))
        XCTAssertTrue(fixture.model.runs.isEmpty)
        XCTAssertEqual(fixture.model.pointCount, 0)
    }

    @MainActor
    func testClearTrailIsLocalOnly() {
        let fixture = SurveyFixture()
        fixture.model.apply(Self.view(min: -90, max: -10))
        fixture.model.append([Self.point(0, -60)])
        fixture.model.clearTrail()
        XCTAssertTrue(fixture.model.runs.isEmpty)
        XCTAssertFalse(fixture.model.trimmed)
        XCTAssertTrue(fixture.sent.isEmpty)
        XCTAssertNotNil(fixture.model.view)
    }

    @MainActor
    func testPointsIgnoredWhileClosed() {
        let fixture = SurveyFixture()
        fixture.model.missionClosed()
        fixture.model.append([Self.point(0, -60)])
        XCTAssertTrue(fixture.model.runs.isEmpty)
        fixture.model.missionOpened()
        fixture.model.append([Self.point(0, -60)])
        XCTAssertEqual(fixture.model.runs.count, 1)
    }

    @MainActor
    func testRecordAndStopSendCommands() async {
        let fixture = SurveyFixture()
        fixture.model.apply(Self.view(min: -90, max: -10, recording: true))
        await fixture.model.toggleRecording()
        fixture.model.apply(Self.view(min: -90, max: -10, recording: false))
        await fixture.model.toggleRecording()
        XCTAssertEqual(fixture.sent, [.stopSurvey, .startSurvey])
        fixture.core.fail(next: .Refused(message: "No position"))
        await fixture.model.toggleRecording()
        XCTAssertEqual(fixture.sink.labels, ["No position"])
        XCTAssertFalse(fixture.model.busy)
    }

    @MainActor
    func testFitCoversTheTrail() {
        let fixture = SurveyFixture()
        fixture.model.fit()
        XCTAssertNil(fixture.model.camera.region)
        fixture.model.append((0..<20).map { Self.point($0, -50) })
        fixture.model.fit()
        let region = fixture.model.camera.region
        XCTAssertEqual(region?.center.latitude ?? 0, 52.5, accuracy: 1e-9)
        XCTAssertEqual(region?.center.longitude ?? 0, 13.4 + 19e-4 / 2, accuracy: 1e-9)
        XCTAssertGreaterThanOrEqual(region?.span.longitudeDelta ?? 0, 19e-4)
    }
}
