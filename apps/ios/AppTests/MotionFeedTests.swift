import CoreMotion
import XCTest

@testable import SDRmm

nonisolated final class OffMainMotionManager: CMMotionManager, @unchecked Sendable {
    override var isDeviceMotionAvailable: Bool { true }

    override func startDeviceMotionUpdates(
        using referenceFrame: CMAttitudeReferenceFrame,
        to queue: OperationQueue,
        withHandler handler: @escaping CMDeviceMotionHandler
    ) {
        nonisolated(unsafe) let handler = handler
        queue.addOperation {
            handler(nil, NSError(domain: CMErrorDomain, code: Int(CMErrorDeviceRequiresMovement.rawValue)))
        }
    }

    override func stopDeviceMotionUpdates() {}
}

@MainActor
final class MotionFeedTests: XCTestCase {
    func testAnUpdateOnTheMotionQueueReachesTheAppWithoutTrapping() {
        let feed = MotionFeed(manager: OffMainMotionManager())
        let failed = expectation(description: "the failure reaches the main actor")
        feed.start(sink: { _ in }, failure: { _ in failed.fulfill() })
        wait(for: [failed], timeout: 5)
        feed.stop()
    }
}
