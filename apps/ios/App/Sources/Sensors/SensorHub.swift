import CoreLocation
import Observation
import SdrmmCore
import os

enum SensorProfile: Equatable {
    case drive, walk
}

nonisolated enum LocationAccess: Equatable, Sendable {
    case unknown, denied, whenInUse, always, restricted
}

struct SensorStatus: Equatable {
    var access: LocationAccess = .unknown
    var precise = true
    var headingAvailable = false
    var motionAvailable = false
    var running = false
    var lastError: String?
}

enum LocationFeedEvent {
    case fix(CLLocation)
    case access(LocationAccess)
    case precise(Bool)
    case unavailable
    case failed(String)
}

protocol LocationFeeding: AnyObject {
    func observeAuthorization(_ sink: @escaping @MainActor (LocationFeedEvent) -> Void)
    func start(profile: SensorProfile, sink: @escaping @MainActor (LocationFeedEvent) -> Void)
    func stop()
    func requestAlways()
    func requestPrecise()
}

protocol HeadingFeeding: AnyObject {
    var available: Bool { get }
    func start(
        sink: @escaping @Sendable (HeadingSample) -> Void,
        failure: @escaping @MainActor (String) -> Void
    )
    func stop()
}

protocol MotionFeeding: AnyObject {
    var available: Bool { get }
    func start(
        sink: @escaping @Sendable (MotionSample) -> Void,
        failure: @escaping @MainActor (String) -> Void
    )
    func stop()
}

@Observable
final class SensorHub {
    private(set) var status = SensorStatus()
    private(set) var profile: SensorProfile?
    private(set) var droppedFixes = 0
    @ObservationIgnored var onFix: (@MainActor (CLLocation) -> Void)?
    @ObservationIgnored private let core: any CoreService
    @ObservationIgnored private let location: any LocationFeeding
    @ObservationIgnored private let heading: any HeadingFeeding
    @ObservationIgnored private let motion: any MotionFeeding
    @ObservationIgnored private var locationError = false

    init(
        core: any CoreService,
        location: any LocationFeeding,
        heading: any HeadingFeeding,
        motion: any MotionFeeding
    ) {
        self.core = core
        self.location = location
        self.heading = heading
        self.motion = motion
        location.observeAuthorization { [weak self] event in
            self?.handle(event)
        }
    }

    func start(profile: SensorProfile) {
        if self.profile == profile {
            return
        }
        if self.profile != nil {
            stop()
        }
        self.profile = profile
        status.running = true
        status.lastError = nil
        locationError = false
        location.start(profile: profile) { [weak self] event in
            self?.handle(event)
        }
        startHeading()
        startMotion()
    }

    func stop() {
        location.stop()
        heading.stop()
        motion.stop()
        profile = nil
        status.running = false
    }

    func requestAlways() {
        location.requestAlways()
    }

    func requestPrecise() {
        location.requestPrecise()
    }

    private func startHeading() {
        status.headingAvailable = heading.available
        guard heading.available else {
            return
        }
        let core = core
        heading.start(
            sink: { core.pushHeading($0) },
            failure: { [weak self] text in self?.failed("heading", text) }
        )
    }

    private func startMotion() {
        status.motionAvailable = motion.available
        guard motion.available else {
            return
        }
        let core = core
        motion.start(
            sink: { core.pushMotion($0) },
            failure: { [weak self] text in
                self?.motion.stop()
                self?.failed("motion", text)
            }
        )
    }

    private func failed(_ sensor: String, _ text: String) {
        Log.sensors.error("\(sensor, privacy: .public) failed: \(text, privacy: .public)")
        status.lastError = text
        locationError = false
    }

    private func locationFailed(_ text: String) {
        failed("location", text)
        locationError = true
    }

    private func handle(_ event: LocationFeedEvent) {
        switch event {
        case .fix(let fix):
            guard let sample = SampleMapping.location(fix) else {
                droppedFixes += 1
                Log.sensors.debug("dropped fix without accuracy, \(self.droppedFixes) so far")
                return
            }
            if locationError {
                status.lastError = nil
                locationError = false
            }
            core.pushLocation(sample)
            Log.sensors.debug("location pushed, accuracy \(sample.hAccM, privacy: .public) m")
            onFix?(fix)
        case .access(let access):
            status.access = access
        case .precise(let precise):
            status.precise = precise
        case .unavailable:
            locationFailed("No location")
        case .failed(let text):
            locationFailed(text)
        }
    }
}
