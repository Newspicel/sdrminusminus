import Foundation
import Observation
import SdrmmCore

enum UnitsChoice: String, CaseIterable, Identifiable {
    case auto, metric, imperial

    var id: String { rawValue }
}

nonisolated enum UnitSystem: Equatable, Sendable {
    case metric, imperial
}

@Observable
final class SettingsStore {
    private enum Key: String {
        case headingMode, mount, mountOffsetDeg, units, voiceOn, voiceID, clicksOn, hapticsOn, phoneName
        case activeServerID, routeNoticeAccepted

        var name: String { "sdrmm." + rawValue }
    }

    @ObservationIgnored private let defaults: UserDefaults
    @ObservationIgnored var onPoseChange: (@MainActor (PoseSettings) -> Void)?
    private var storedHeadingMode: HeadingMode
    private var storedMount: Mount
    private var storedOffset: Double
    private var storedUnits: UnitsChoice
    private var storedVoiceOn: Bool
    private var storedVoiceID: String?
    private var storedClicksOn: Bool
    private var storedHapticsOn: Bool
    private var storedPhoneName: String
    private var storedActiveServerID: String?
    private var storedRouteNoticeAccepted: Bool

    init(defaults: UserDefaults) {
        self.defaults = defaults
        storedHeadingMode = Self.headingMode(defaults.string(forKey: Key.headingMode.name))
        storedMount = defaults.string(forKey: Key.mount.name) == "upright" ? .upright : .flat
        storedOffset = Self.wrap(defaults.double(forKey: Key.mountOffsetDeg.name))
        storedUnits = UnitsChoice(rawValue: defaults.string(forKey: Key.units.name) ?? "") ?? .auto
        storedVoiceOn = defaults.object(forKey: Key.voiceOn.name) as? Bool ?? true
        storedVoiceID = defaults.string(forKey: Key.voiceID.name)
        storedClicksOn = defaults.object(forKey: Key.clicksOn.name) as? Bool ?? true
        storedHapticsOn = defaults.object(forKey: Key.hapticsOn.name) as? Bool ?? true
        storedPhoneName = defaults.string(forKey: Key.phoneName.name) ?? "iPhone"
        storedActiveServerID = defaults.string(forKey: Key.activeServerID.name)
        storedRouteNoticeAccepted = defaults.bool(forKey: Key.routeNoticeAccepted.name)
    }

    var headingMode: HeadingMode {
        get { storedHeadingMode }
        set {
            storedHeadingMode = newValue
            defaults.set(Self.name(newValue), forKey: Key.headingMode.name)
            onPoseChange?(poseSettings)
        }
    }

    var mount: Mount {
        get { storedMount }
        set {
            storedMount = newValue
            defaults.set(newValue == .upright ? "upright" : "flat", forKey: Key.mount.name)
            onPoseChange?(poseSettings)
        }
    }

    var mountOffsetDeg: Double {
        get { storedOffset }
        set {
            storedOffset = Self.wrap(newValue)
            defaults.set(storedOffset, forKey: Key.mountOffsetDeg.name)
            onPoseChange?(poseSettings)
        }
    }

    var units: UnitsChoice {
        get { storedUnits }
        set {
            storedUnits = newValue
            defaults.set(newValue.rawValue, forKey: Key.units.name)
        }
    }

    var voiceOn: Bool {
        get { storedVoiceOn }
        set {
            storedVoiceOn = newValue
            defaults.set(newValue, forKey: Key.voiceOn.name)
        }
    }

    var voiceID: String? {
        get { storedVoiceID }
        set {
            storedVoiceID = newValue
            defaults.set(newValue, forKey: Key.voiceID.name)
        }
    }

    var clicksOn: Bool {
        get { storedClicksOn }
        set {
            storedClicksOn = newValue
            defaults.set(newValue, forKey: Key.clicksOn.name)
        }
    }

    var hapticsOn: Bool {
        get { storedHapticsOn }
        set {
            storedHapticsOn = newValue
            defaults.set(newValue, forKey: Key.hapticsOn.name)
        }
    }

    var phoneName: String {
        get { storedPhoneName }
        set {
            storedPhoneName = newValue
            defaults.set(newValue, forKey: Key.phoneName.name)
        }
    }

    var activeServerID: String? {
        get { storedActiveServerID }
        set {
            storedActiveServerID = newValue
            defaults.set(newValue, forKey: Key.activeServerID.name)
        }
    }

    var routeNoticeAccepted: Bool {
        get { storedRouteNoticeAccepted }
        set {
            storedRouteNoticeAccepted = newValue
            defaults.set(newValue, forKey: Key.routeNoticeAccepted.name)
        }
    }

    var poseSettings: PoseSettings {
        PoseSettings(headingMode: headingMode, mount: mount, mountOffsetDeg: mountOffsetDeg, sharePose: true)
    }

    var unitSystem: UnitSystem {
        Self.unitSystem(units, locale: .current)
    }

    static func unitSystem(_ choice: UnitsChoice, locale: Locale) -> UnitSystem {
        switch choice {
        case .metric: .metric
        case .imperial: .imperial
        case .auto: locale.measurementSystem == .metric ? .metric : .imperial
        }
    }

    static func wrap(_ degrees: Double) -> Double {
        guard degrees.isFinite else {
            return 0
        }
        let shifted = (degrees + 180).truncatingRemainder(dividingBy: 360)
        return (shifted < 0 ? shifted + 360 : shifted) - 180
    }

    private static func headingMode(_ name: String?) -> HeadingMode {
        switch name {
        case "compass": .compass
        case "course": .course
        default: .auto
        }
    }

    private static func name(_ mode: HeadingMode) -> String {
        switch mode {
        case .auto: "auto"
        case .compass: "compass"
        case .course: "course"
        }
    }
}
