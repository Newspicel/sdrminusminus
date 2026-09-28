import SdrmmCore
import XCTest

@testable import SDRmm

@MainActor
final class SettingsStoreTests: XCTestCase {
    func testDefaults() {
        let settings = SettingsStore(defaults: TestDefaults.make())
        XCTAssertEqual(settings.headingMode, .auto)
        XCTAssertEqual(settings.mount, .flat)
        XCTAssertEqual(settings.mountOffsetDeg, 0)
        XCTAssertEqual(settings.units, .auto)
        XCTAssertTrue(settings.voiceOn)
        XCTAssertNil(settings.voiceID)
        XCTAssertTrue(settings.clicksOn)
        XCTAssertTrue(settings.hapticsOn)
        XCTAssertEqual(settings.phoneName, "iPhone")
        XCTAssertNil(settings.activeServerID)
        XCTAssertEqual(
            settings.poseSettings,
            PoseSettings(headingMode: .auto, mount: .flat, mountOffsetDeg: 0, sharePose: true)
        )
    }

    func testPersistsAcrossInstances() {
        let defaults = TestDefaults.make()
        let first = SettingsStore(defaults: defaults)
        first.headingMode = .course
        first.mount = .upright
        first.mountOffsetDeg = 12
        first.units = .imperial
        first.voiceOn = false
        first.voiceID = "voice"
        first.clicksOn = false
        first.hapticsOn = false
        first.phoneName = "Car phone"
        first.activeServerID = "s1"
        let second = SettingsStore(defaults: defaults)
        XCTAssertEqual(second.headingMode, .course)
        XCTAssertEqual(second.mount, .upright)
        XCTAssertEqual(second.mountOffsetDeg, 12)
        XCTAssertEqual(second.units, .imperial)
        XCTAssertFalse(second.voiceOn)
        XCTAssertEqual(second.voiceID, "voice")
        XCTAssertFalse(second.clicksOn)
        XCTAssertFalse(second.hapticsOn)
        XCTAssertEqual(second.phoneName, "Car phone")
        XCTAssertEqual(second.activeServerID, "s1")
    }

    func testOffsetWraps() {
        let settings = SettingsStore(defaults: TestDefaults.make())
        settings.mountOffsetDeg = 190
        XCTAssertEqual(settings.mountOffsetDeg, -170)
        settings.mountOffsetDeg = -181
        XCTAssertEqual(settings.mountOffsetDeg, 179)
        settings.mountOffsetDeg = 180
        XCTAssertEqual(settings.mountOffsetDeg, -180)
        settings.mountOffsetDeg = .nan
        XCTAssertEqual(settings.mountOffsetDeg, 0)
    }

    func testUnitSystemForLocale() {
        XCTAssertEqual(SettingsStore.unitSystem(.auto, locale: Locale(identifier: "de_DE")), .metric)
        XCTAssertEqual(SettingsStore.unitSystem(.auto, locale: Locale(identifier: "en_US")), .imperial)
        XCTAssertEqual(SettingsStore.unitSystem(.auto, locale: Locale(identifier: "en_GB")), .imperial)
        XCTAssertEqual(SettingsStore.unitSystem(.metric, locale: Locale(identifier: "en_US")), .metric)
        XCTAssertEqual(SettingsStore.unitSystem(.imperial, locale: Locale(identifier: "de_DE")), .imperial)
    }

    func testPoseChangeCallback() {
        let settings = SettingsStore(defaults: TestDefaults.make())
        var received: [PoseSettings] = []
        settings.onPoseChange = { received.append($0) }
        settings.mount = .upright
        XCTAssertEqual(received.count, 1)
        XCTAssertEqual(received.first?.mount, .upright)
        settings.units = .metric
        XCTAssertEqual(received.count, 1)
    }
}
