import CoreLocation
import SdrmmCore
import Synchronization

nonisolated final class HeadingFeed: NSObject, HeadingFeeding, CLLocationManagerDelegate {
    private let manager: CLLocationManager
    private let sink = Mutex<(@Sendable (HeadingSample) -> Void)?>(nil)
    private let failure = Mutex<(@MainActor (String) -> Void)?>(nil)

    override init() {
        manager = CLLocationManager()
        super.init()
        manager.headingFilter = 1
        manager.delegate = self
    }

    var available: Bool { CLLocationManager.headingAvailable() }

    func start(
        sink: @escaping @Sendable (HeadingSample) -> Void,
        failure: @escaping @MainActor (String) -> Void
    ) {
        self.sink.withLock { $0 = sink }
        self.failure.withLock { $0 = failure }
        manager.startUpdatingHeading()
    }

    func stop() {
        manager.stopUpdatingHeading()
        sink.withLock { $0 = nil }
        failure.withLock { $0 = nil }
    }

    func locationManager(_ manager: CLLocationManager, didUpdateHeading newHeading: CLHeading) {
        let sample = SampleMapping.heading(newHeading)
        sink.withLock { $0 }?(sample)
    }

    func locationManagerShouldDisplayHeadingCalibration(_ manager: CLLocationManager) -> Bool {
        false
    }

    func locationManager(_ manager: CLLocationManager, didFailWithError error: Error) {
        let text = error.localizedDescription
        guard
            let report = failure.withLock({ current in
                defer { current = nil }
                return current
            })
        else {
            return
        }
        Task { @MainActor in report(text) }
    }
}
