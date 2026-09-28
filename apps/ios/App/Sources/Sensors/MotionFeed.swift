import CoreMotion
import Foundation
import SdrmmCore
import Synchronization

nonisolated final class MotionFeed: MotionFeeding {
    private let manager: CMMotionManager
    private let rateHz: Double
    private let queue: OperationQueue

    init(manager: CMMotionManager = CMMotionManager(), rateHz: Double = 50) {
        self.manager = manager
        self.rateHz = rateHz
        queue = OperationQueue()
        queue.maxConcurrentOperationCount = 1
        queue.qualityOfService = .userInitiated
        queue.name = "dev.newspicel.sdrmm.motion"
    }

    var available: Bool { manager.isDeviceMotionAvailable }

    func start(
        sink: @escaping @Sendable (MotionSample) -> Void,
        failure: @escaping @MainActor (String) -> Void
    ) {
        manager.deviceMotionUpdateInterval = 1 / rateHz
        let trueNorth = CMMotionManager.availableAttitudeReferenceFrames().contains(.xTrueNorthZVertical)
        let reference: CMAttitudeReferenceFrame =
            trueNorth ? .xTrueNorthZVertical : .xArbitraryCorrectedZVertical
        let frame: MotionFrame = trueNorth ? .trueNorth : .arbitrary
        let once = FirstTime()
        manager.startDeviceMotionUpdates(using: reference, to: queue) { motion, error in
            if let error {
                if once.claim() {
                    let text = error.localizedDescription
                    Task { @MainActor in failure(text) }
                }
                return
            }
            guard let motion else {
                return
            }
            let sample = SampleMapping.motion(
                motion,
                frame: frame,
                uptimeNow: ProcessInfo.processInfo.systemUptime,
                wallNow: Date()
            )
            sink(sample)
        }
    }

    func stop() {
        manager.stopDeviceMotionUpdates()
    }
}

nonisolated final class FirstTime: Sendable {
    private let used = Atomic<Bool>(false)

    func claim() -> Bool {
        !used.exchange(true, ordering: .relaxed)
    }
}
