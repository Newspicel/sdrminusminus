import SdrmmCore
import XCTest

@testable import SDRmm

@MainActor
final class PairModelTests: XCTestCase {
    private let link =
        "sdrmm://pair?h=10.0.0.2:8443&c=48210937&fp=\(FakeScenarios.fingerprint)&p=1"
    private let nearby = DiscoveredServer(name: "Lab Pi", hosts: ["10.0.0.2:8443"], txt: ["id": "s1"])

    private func make(_ core: FakeCore = FakeCore(scenario: .fresh, ticking: false))
        -> (PairModel, FakeCore, SettingsStore, FakeBrowser, PairedRecorder)
    {
        let settings = SettingsStore(defaults: TestDefaults.make())
        let browser = FakeBrowser()
        let recorder = PairedRecorder()
        let model = PairModel(core: core, browser: browser, settings: settings) { server in
            recorder.servers.append(server)
        }
        return (model, core, settings, browser, recorder)
    }

    func testQrLinkGoesToConfirm() {
        let (model, core, _, _, _) = make()
        model.scanned(link)
        guard case .confirm(let offer) = model.step else {
            return XCTFail("expected the trust step, got \(model.step)")
        }
        XCTAssertEqual(offer.fingerprintShort, FakeScenarios.fingerprintShort)
        XCTAssertEqual(core.calls, [.parse(link)])
        XCTAssertNil(model.error)
    }

    func testNonSdrmmPayloadRejected() {
        let (model, core, _, _, _) = make()
        model.scanned("https://x")
        XCTAssertEqual(model.error, "Bad QR")
        XCTAssertEqual(model.step, .choose)
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testCodeMustBeEightDigits() {
        let (model, core, _, _, _) = make()
        model.choose(nearby)
        model.code = "12a4"
        model.submitCode()
        XCTAssertEqual(model.error, "8 digits")
        model.code = "1234567"
        model.submitCode()
        XCTAssertEqual(model.error, "8 digits")
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testWrongCodeLabel() {
        let (model, _, _, _, _) = make()
        model.choose(nearby)
        model.code = "00000000"
        model.submitCode()
        XCTAssertEqual(model.error, "Wrong code")
        XCTAssertEqual(model.step, .code(nearby))
    }

    func testDiscoveryOfferNeedsTrust() {
        let (model, _, _, _, _) = make()
        model.choose(nearby)
        model.code = FakeScenarios.code
        model.submitCode()
        guard case .confirm(let offer) = model.step else {
            return XCTFail("expected the trust step, got \(model.step)")
        }
        XCTAssertEqual(offer.hosts, nearby.hosts)
    }

    func testTrustPairsWithPhoneName() async {
        let (model, core, settings, _, recorder) = make()
        settings.phoneName = "Field phone"
        model.scanned(link)
        guard case .confirm(let offer) = model.step else {
            return XCTFail("expected the trust step, got \(model.step)")
        }
        await model.trust()
        XCTAssertTrue(core.calls.contains(.pair(offer, "Field phone")))
        XCTAssertEqual(recorder.servers, [FakeScenarios.server()])
        XCTAssertEqual(model.step, .choose)
    }

    func testUnreachableServerStaysOnTrustWithError() async {
        let (model, core, _, _, recorder) = make()
        model.scanned(link)
        core.fail(next: .Unreachable(hosts: ["10.0.0.2:8443"]))
        await model.trust()
        XCTAssertEqual(model.error, "No answer")
        XCTAssertTrue(recorder.servers.isEmpty)
        guard case .confirm = model.step else {
            return XCTFail("expected the trust step, got \(model.step)")
        }
    }

    func testExpiredCodeLeavesTheTrustStep() async {
        let (model, core, _, _, recorder) = make()
        model.scanned(link)
        core.fail(next: .CodeExpired)
        await model.trust()
        XCTAssertEqual(model.error, "Code expired")
        XCTAssertEqual(model.step, .choose)
        XCTAssertTrue(recorder.servers.isEmpty)
    }

    func testManualOfferNeedsTrust() async {
        let (model, _, _, _, _) = make()
        model.address = "10.0.0.2:8443"
        model.code = FakeScenarios.code
        await model.submitManual()
        guard case .confirm(let offer) = model.step else {
            return XCTFail("expected the trust step, got \(model.step)")
        }
        XCTAssertEqual(offer.fingerprintShort, FakeScenarios.fingerprintShort)
        XCTAssertEqual(offer.hosts, ["10.0.0.2:8443"])
    }

    func testManualNeedsAddress() async {
        let (model, core, _, _, _) = make()
        model.code = FakeScenarios.code
        await model.submitManual()
        XCTAssertEqual(model.error, "Address missing")
        XCTAssertTrue(core.calls.isEmpty)
    }

    func testCancelResets() {
        let (model, _, _, _, _) = make()
        model.scanned("https://x")
        model.cancel()
        XCTAssertEqual(model.step, .choose)
        XCTAssertNil(model.error)
    }

    func testNearbyListAndBrowseError() {
        let (model, _, _, browser, _) = make()
        model.appear()
        browser.found([nearby])
        XCTAssertEqual(model.nearby, [nearby])
        browser.fail("Local network off")
        XCTAssertEqual(model.browseError, "Local network off")
        model.disappear()
        XCTAssertEqual(browser.stops, 1)
    }
}

@MainActor
final class PairedRecorder {
    var servers: [SavedServer] = []
}
