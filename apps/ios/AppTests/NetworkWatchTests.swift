import XCTest

@testable import SDRmm

@MainActor
final class FakeNetworkWatch: NetworkWatching {
    private(set) var starts = 0
    private(set) var stops = 0
    private var onChange: (@MainActor () -> Void)?

    func start(onChange: @escaping @MainActor () -> Void) {
        starts += 1
        self.onChange = onChange
    }

    func stop() {
        stops += 1
    }

    func change() {
        onChange?()
    }
}

@MainActor
final class NetworkWatchTests: XCTestCase {
    private func path(_ usable: Bool, _ interfaces: [String], _ gateways: [String] = ["192.168.1.1"])
        -> PathSummary
    {
        PathSummary(usable: usable, interfaces: interfaces, gateways: gateways)
    }

    func testTheFirstPathIsNotAChange() {
        XCTAssertFalse(PathSummary.matters(from: nil, to: path(true, ["en0"])))
    }

    func testANewUsablePathIsAChange() {
        XCTAssertTrue(PathSummary.matters(from: path(true, ["en0"]), to: path(true, ["pdp_ip0"])))
        XCTAssertTrue(PathSummary.matters(from: path(false, []), to: path(true, ["en0"])))
        XCTAssertTrue(
            PathSummary.matters(
                from: path(true, ["en0"], ["10.0.0.1"]),
                to: path(true, ["en0"], ["10.0.1.1"])
            )
        )
    }

    func testALostOrSamePathIsNotAChange() {
        XCTAssertFalse(PathSummary.matters(from: path(true, ["en0"]), to: path(false, [])))
        XCTAssertFalse(PathSummary.matters(from: path(true, ["en0"]), to: path(true, ["en0"])))
    }

    func testANetworkChangeReachesTheCore() async {
        let core = FakeCore(scenario: .paired, ticking: false)
        let network = FakeNetworkWatch()
        let feeds = FakeFeeds()
        let session = SilentAudioSession()
        let model = AppModel(
            core: core,
            settings: SettingsStore(defaults: TestDefaults.make()),
            routes: FakeRouteProvider(plans: []),
            speech: SpeechRecorder(),
            clicks: ClickRecorder(),
            browser: FakeBrowser(),
            sensors: feeds.hub(core: core),
            notifier: NotifierRecorder(),
            audio: AudioSessionController(session: session, center: NotificationCenter()),
            network: network
        )
        let running = Task { await model.run() }
        while network.starts == 0 {
            await Task.yield()
        }
        network.change()
        XCTAssertEqual(core.calls.filter { $0 == .networkChanged }.count, 1)
        core.finish()
        await running.value
        XCTAssertEqual(network.stops, 1)
    }
}
