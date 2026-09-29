import AVFAudio
import Foundation
import SdrmmCore
import Synchronization

@testable import SDRmm

@MainActor
final class FakeBrowser: BonjourBrowsing {
    private(set) var starts = 0
    private(set) var stops = 0
    private var onChange: (@MainActor ([DiscoveredServer]) -> Void)?
    private var onError: (@MainActor (String) -> Void)?

    func start(
        onChange: @escaping @MainActor ([DiscoveredServer]) -> Void,
        onError: @escaping @MainActor (String) -> Void
    ) {
        starts += 1
        self.onChange = onChange
        self.onError = onError
    }

    func stop() {
        stops += 1
    }

    func found(_ servers: [DiscoveredServer]) {
        onChange?(servers)
    }

    func fail(_ text: String) {
        onError?(text)
    }
}

@MainActor
final class NotifierRecorder: RetargetNotifying {
    private(set) var authorizations = 0
    private(set) var posted: [(RetargetNotice, String)] = []

    func requestAuthorization() async {
        authorizations += 1
    }

    func post(_ notice: RetargetNotice, distance: String) {
        posted.append((notice, distance))
    }
}

final class SilentAudioSession: AudioSessionPort {
    private struct State {
        var active: [Bool] = []
        var failActivation = false
    }

    private let state = Mutex(State())

    var active: [Bool] { state.withLock { $0.active } }

    var failActivation: Bool {
        get { state.withLock { $0.failActivation } }
        set { state.withLock { $0.failActivation = newValue } }
    }

    func setCategory(
        _ category: AVAudioSession.Category,
        mode: AVAudioSession.Mode,
        options: AVAudioSession.CategoryOptions
    ) throws {}

    func setActive(_ active: Bool, options: AVAudioSession.SetActiveOptions) throws {
        let refused = state.withLock { state in
            let refused = active && state.failActivation
            if !refused {
                state.active.append(active)
            }
            return refused
        }
        if refused {
            throw NotBuilt(feature: "Audio")
        }
    }
}

enum TestDefaults {
    static func make() -> UserDefaults {
        let suite = "dev.newspicel.sdrmm.test.\(UUID().uuidString)"
        let defaults = UserDefaults(suiteName: suite) ?? .standard
        defaults.removePersistentDomain(forName: suite)
        return defaults
    }
}

@MainActor
struct Harness {
    let core: FakeCore
    let settings: SettingsStore
    let feeds = FakeFeeds()
    let browser = FakeBrowser()
    let clicks = ClickRecorder()
    let speech = SpeechRecorder()
    let notifier = NotifierRecorder()
    let session = SilentAudioSession()
    let center = NotificationCenter()
    let model: AppModel

    init(scenario: FakeCore.Scenario = .paired, defaults: UserDefaults = TestDefaults.make()) {
        core = FakeCore(scenario: scenario, ticking: false)
        settings = SettingsStore(defaults: defaults)
        model = AppModel(
            core: core,
            settings: settings,
            routes: FakeRouteProvider(plans: []),
            speech: speech,
            clicks: clicks,
            browser: browser,
            sensors: feeds.hub(core: core),
            notifier: notifier,
            audio: AudioSessionController(session: session, center: center)
        )
    }

    func start(activeServer: String? = FakeScenarios.server().id, events: [CoreEvent] = []) async {
        settings.activeServerID = activeServer
        for event in events {
            core.emit(event)
        }
        core.finish()
        await model.run()
    }
}
