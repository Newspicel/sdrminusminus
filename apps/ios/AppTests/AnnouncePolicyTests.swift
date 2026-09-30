import XCTest

@testable import SDRmm

final class AnnouncePolicyTests: XCTestCase {
    private let plan = Fixtures.steppedPlan()

    func testFarThenNearOnce() {
        var policy = AnnouncePolicy()
        let far = policy.prompts(plan: plan, position: Fixtures.position(next: 2, toNextM: 280), speedMps: 10)
        XCTAssertEqual(far, [Prompt(step: 2, stage: .far, toNextM: 280)])
        XCTAssertEqual(
            policy.prompts(plan: plan, position: Fixtures.position(next: 2, toNextM: 270), speedMps: 10),
            []
        )
        XCTAssertEqual(
            policy.prompts(plan: plan, position: Fixtures.position(next: 2, toNextM: 90), speedMps: 10),
            []
        )
        let near = policy.prompts(plan: plan, position: Fixtures.position(next: 2, toNextM: 45), speedMps: 10)
        XCTAssertEqual(near, [Prompt(step: 2, stage: .near, toNextM: 45)])
        XCTAssertEqual(
            policy.prompts(plan: plan, position: Fixtures.position(next: 2, toNextM: 20), speedMps: 10),
            []
        )
    }

    func testNearOnlyWhenStartingClose() {
        var policy = AnnouncePolicy()
        let first = policy.prompts(plan: plan, position: Fixtures.position(next: 1, toNextM: 30), speedMps: 0)
        XCTAssertEqual(first.map(\.stage), [.near])
        XCTAssertEqual(
            policy.prompts(plan: plan, position: Fixtures.position(next: 1, toNextM: 10), speedMps: 0),
            []
        )
    }

    func testThresholdsScaleWithSpeed() {
        XCTAssertEqual(AnnouncePolicy.farM(speedMps: 30), 900)
        XCTAssertEqual(AnnouncePolicy.nearM(speedMps: 30), 150)
        XCTAssertEqual(AnnouncePolicy.farM(speedMps: 0), 250)
        XCTAssertEqual(AnnouncePolicy.nearM(speedMps: 0), 40)
        XCTAssertEqual(AnnouncePolicy.farM(speedMps: 80), 1_000)
        XCTAssertEqual(AnnouncePolicy.nearM(speedMps: -.infinity), 40)
    }

    func testResetForNewPlan() {
        var policy = AnnouncePolicy()
        let position = Fixtures.position(next: 1, toNextM: 30)
        XCTAssertEqual(policy.prompts(plan: plan, position: position, speedMps: 0).count, 1)
        XCTAssertEqual(policy.prompts(plan: plan, position: position, speedMps: 0).count, 0)
        let other = Fixtures.steppedPlan()
        XCTAssertEqual(policy.prompts(plan: other, position: position, speedMps: 0).count, 1)
        policy.reset()
        XCTAssertEqual(policy.prompts(plan: other, position: position, speedMps: 0).count, 1)
    }

    func testPromptTexts() {
        let far = Prompt(step: 2, stage: .far, toNextM: 300)
        XCTAssertEqual(PromptText.text(far, plan: plan, units: .metric), "In 300 meters, Step 2")
        let near = Prompt(step: 2, stage: .near, toNextM: 30)
        XCTAssertEqual(PromptText.text(near, plan: plan, units: .metric), "Step 2")
        XCTAssertEqual(PromptText.instruction(plan, step: 0), "Continue")
        XCTAssertEqual(PromptText.retarget(distanceM: 1_200, units: .metric), "New target, 1.2 kilometers")
        XCTAssertEqual(PromptText.retarget(distanceM: nil, units: .metric), "New target")
    }

    func testSpeechQueueDropsStalePrompts() {
        var queue = SpeechQueue()
        let start = Date(timeIntervalSince1970: 0)
        XCTAssertEqual(queue.enqueue("one", at: start), [])
        XCTAssertEqual(queue.next(), "one")
        XCTAssertEqual(queue.enqueue("two", at: start), [])
        XCTAssertNil(queue.next())
        XCTAssertEqual(queue.enqueue("three", at: start.addingTimeInterval(6)), ["two"])
        queue.finished()
        XCTAssertEqual(queue.next(), "three")
        queue.interrupt()
        XCTAssertTrue(queue.waiting.isEmpty)
        queue.finished()
        XCTAssertTrue(queue.idle)
    }
}
