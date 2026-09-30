import Foundation
import Observation
import SdrmmCore
import SwiftUI
import os

enum AppRuntime {
    private static let boot = Boot.make()
    static let model: AppModel = boot.model
    static let coreFailure: String? = boot.failure
    static let demo = DemoSession()
    static var environment: [String: String] { ProcessInfo.processInfo.environment }
    static var isUnitTestHost: Bool { NSClassFromString("XCTestCase") != nil }
    static var isUITest: Bool { environment["SDRMM_UITEST"] == "1" }

    static func start() {
        guard coreFailure == nil else {
            return
        }
        model.start()
    }
}

private struct Boot {
    let model: AppModel
    let failure: String?

    static func make() -> Boot {
        #if DEBUG
            if let scenario = AppRuntime.environment["SDRMM_FAKE_CORE"].flatMap(FakeCore.Scenario.init) {
                return fake(scenario)
            }
            if AppRuntime.environment["SDRMM_E2E"] == "1" {
                return endToEnd()
            }
        #endif
        if AppRuntime.isUnitTestHost {
            return Boot(
                model: AppAssembly.model(core: FakeCore(scenario: .fresh), settings: scratch("test")),
                failure: nil
            )
        }
        return live(vault: KeychainVault(), settings: SettingsStore(defaults: .standard))
    }

    private static func live(vault: KeychainVault, settings: SettingsStore) -> Boot {
        do {
            let core = try LiveCore(config: try config(), vault: vault)
            return Boot(model: AppAssembly.model(core: core, settings: settings), failure: nil)
        } catch {
            Log.core.fault("core failed: \(CoreErrorText.detail(error), privacy: .public)")
            let inert = AppAssembly.model(core: FakeCore(scenario: .fresh), settings: scratch("inert"))
            return Boot(model: inert, failure: CoreErrorText.detail(error))
        }
    }

    #if DEBUG
        private static func endToEnd() -> Boot {
            let vault = KeychainVault(service: "dev.newspicel.sdrmm.e2e")
            let settings = scratch("e2e")
            quiet(settings)
            do {
                for key in try vault.keys() {
                    try vault.delete(key: key)
                }
            } catch {
                let inert = AppAssembly.model(core: FakeCore(scenario: .fresh), settings: scratch("inert"))
                return Boot(model: inert, failure: CoreErrorText.detail(error))
            }
            return live(vault: vault, settings: settings)
        }

        private static func quiet(_ settings: SettingsStore) {
            settings.voiceOn = false
            settings.hapticsOn = false
            settings.clicksOn = false
        }

        private static func fake(_ scenario: FakeCore.Scenario) -> Boot {
            let settings = scratch("fake")
            if scenario != .fresh {
                settings.activeServerID = FakeScenarios.server().id
            }
            if AppRuntime.isUITest {
                quiet(settings)
            }
            let ticking = AppRuntime.environment["SDRMM_UITEST_STATIC"] != "1"
            let core = FakeCore(scenario: scenario, ticking: ticking)
            return Boot(model: AppAssembly.model(core: core, settings: settings), failure: nil)
        }
    #endif

    private static func config() throws -> CoreConfig {
        let support = try FileManager.default.url(
            for: .applicationSupportDirectory,
            in: .userDomainMask,
            appropriateFor: nil,
            create: true
        )
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String
        return CoreConfig(
            appVersion: version ?? "0.0.0",
            platform: .ios,
            deviceModel: deviceModel(),
            dataDir: support.path(percentEncoded: false)
        )
    }

    private static func deviceModel() -> String {
        if let simulated = AppRuntime.environment["SIMULATOR_MODEL_IDENTIFIER"] {
            return simulated
        }
        var info = utsname()
        uname(&info)
        return withUnsafeBytes(of: info.machine) { bytes in
            String(decoding: bytes.prefix { $0 != 0 }, as: UTF8.self)
        }
    }
}

func scratch(_ name: String) -> SettingsStore {
    let suite = "dev.newspicel.sdrmm.\(name)"
    guard let defaults = UserDefaults(suiteName: suite) else {
        return SettingsStore(defaults: .standard)
    }
    defaults.removePersistentDomain(forName: suite)
    return SettingsStore(defaults: defaults)
}

enum AppAssembly {
    static func model(core: any CoreService, settings: SettingsStore) -> AppModel {
        let audio = AudioSessionController()
        let sensors = SensorHub(
            core: core,
            location: LocationFeed(),
            heading: HeadingFeed(),
            motion: MotionFeed()
        )
        return AppModel(
            core: core,
            settings: settings,
            routes: routes(),
            speech: SpeechPrompter(settings: settings, session: audio),
            clicks: ClickPlayer(),
            browser: BonjourBrowser(),
            sensors: sensors,
            notifier: RetargetNotifier(),
            audio: audio
        )
    }

    private static func routes() -> any RouteProviding {
        #if DEBUG
            if AppRuntime.environment["SDRMM_FAKE_ROUTES"] == "1" {
                return FakeRouteProvider(plans: [FakeScenarios.plan()])
            }
        #endif
        return MapKitRouteProvider()
    }
}

@Observable
final class DemoSession {
    private(set) var model: AppModel?

    func start() {
        let settings = scratch("demo")
        settings.activeServerID = FakeScenarios.server().id
        let model = AppAssembly.model(core: FakeCore(scenario: .demo), settings: settings)
        model.demoExit = { [weak self] in self?.leave() }
        self.model = model
    }

    func leave() {
        guard let model else {
            return
        }
        model.closeMission()
        (model.core as? FakeCore)?.finish()
        self.model = nil
    }
}
