import SdrmmCore
import XCTest

@testable import SDRmm

final class HuntHapticPolicyTests: XCTestCase {
    func testWarmerTransitionCuesIncrease() {
        var policy = HuntHapticPolicy()
        XCTAssertNil(policy.cue(trend: .waiting, strength: 0.1, at: 0))
        XCTAssertEqual(policy.cue(trend: .warmer, strength: 0.2, at: 1), .increase)
    }

    func testColderTransitionCuesDecrease() {
        var policy = HuntHapticPolicy()
        XCTAssertEqual(policy.cue(trend: .warmer, strength: 0.2, at: 0), .increase)
        XCTAssertEqual(policy.cue(trend: .colder, strength: 0.1, at: 1), .decrease)
        XCTAssertNil(policy.cue(trend: .onTop, strength: 0.5, at: 2))
        XCTAssertNil(policy.cue(trend: .waiting, strength: 0.5, at: 3))
    }

    func testRepeatedWarmerNoCue() {
        var policy = HuntHapticPolicy()
        XCTAssertEqual(policy.cue(trend: .warmer, strength: 0.2, at: 0), .increase)
        XCTAssertNil(policy.cue(trend: .warmer, strength: 0.3, at: 1))
        XCTAssertNil(policy.cue(trend: .warmer, strength: 0.4, at: 2))
    }

    func testMinimumInterval() {
        var policy = HuntHapticPolicy()
        XCTAssertEqual(HuntHapticPolicy.minimumInterval, 0.7)
        XCTAssertEqual(policy.cue(trend: .warmer, strength: 0.2, at: 10), .increase)
        XCTAssertNil(policy.cue(trend: .colder, strength: 0.1, at: 10.5))
        XCTAssertEqual(policy.cue(trend: .warmer, strength: 0.2, at: 10.8), .increase)
        XCTAssertNil(policy.cue(trend: .warmer, strength: 0.2, at: 11.2))
        XCTAssertEqual(policy.cue(trend: .colder, strength: 0.1, at: 11.6), .decrease)
    }

    func testSuccessRearmsBelowPointEight() {
        var policy = HuntHapticPolicy()
        XCTAssertEqual(policy.cue(trend: .onTop, strength: 0.95, at: 0), .success)
        XCTAssertNil(policy.cue(trend: .onTop, strength: 0.96, at: 1))
        XCTAssertNil(policy.cue(trend: .onTop, strength: 0.85, at: 2))
        XCTAssertNil(policy.cue(trend: .onTop, strength: 0.92, at: 3))
        XCTAssertNil(policy.cue(trend: .onTop, strength: 0.7, at: 4))
        XCTAssertEqual(policy.cue(trend: .onTop, strength: 0.92, at: 5), .success)
    }

    func testSuccessInsideIntervalStaysArmed() {
        var policy = HuntHapticPolicy()
        XCTAssertEqual(policy.cue(trend: .warmer, strength: 0.5, at: 0), .increase)
        XCTAssertNil(policy.cue(trend: .warmer, strength: 0.95, at: 0.3))
        XCTAssertEqual(policy.cue(trend: .warmer, strength: 0.95, at: 0.8), .success)
    }

    func testResetForgetsHistory() {
        var policy = HuntHapticPolicy()
        XCTAssertEqual(policy.cue(trend: .warmer, strength: 0.2, at: 0), .increase)
        policy.reset()
        XCTAssertEqual(policy.cue(trend: .warmer, strength: 0.2, at: 0.1), .increase)
    }
}
