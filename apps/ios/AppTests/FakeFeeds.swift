import SdrmmCore

@testable import SDRmm

@MainActor
final class FakeLocationFeed: LocationFeeding {
    private(set) var started: [SensorProfile] = []
    private(set) var stops = 0
    private(set) var alwaysRequests = 0
    private(set) var preciseRequests = 0
    private var sink: (@MainActor (LocationFeedEvent) -> Void)?
    private var statusSink: (@MainActor (LocationFeedEvent) -> Void)?

    func observeAuthorization(_ sink: @escaping @MainActor (LocationFeedEvent) -> Void) {
        statusSink = sink
    }

    func start(profile: SensorProfile, sink: @escaping @MainActor (LocationFeedEvent) -> Void) {
        started.append(profile)
        self.sink = sink
    }

    func stop() {
        stops += 1
        sink = nil
    }

    func requestAlways() {
        alwaysRequests += 1
    }

    func requestPrecise() {
        preciseRequests += 1
    }

    func send(_ event: LocationFeedEvent) {
        (sink ?? statusSink)?(event)
    }
}

@MainActor
final class FakeHeadingFeed: HeadingFeeding {
    var available = true
    private(set) var starts = 0
    private(set) var stops = 0
    private var sink: (@Sendable (HeadingSample) -> Void)?
    private var failure: (@MainActor (String) -> Void)?

    func start(
        sink: @escaping @Sendable (HeadingSample) -> Void,
        failure: @escaping @MainActor (String) -> Void
    ) {
        starts += 1
        self.sink = sink
        self.failure = failure
    }

    func stop() {
        stops += 1
        sink = nil
    }

    func send(_ sample: HeadingSample) {
        sink?(sample)
    }

    func fail(_ text: String) {
        failure?(text)
    }
}

@MainActor
final class FakeMotionFeed: MotionFeeding {
    var available = true
    private(set) var starts = 0
    private(set) var stops = 0
    private var sink: (@Sendable (MotionSample) -> Void)?
    private var failure: (@MainActor (String) -> Void)?

    func start(
        sink: @escaping @Sendable (MotionSample) -> Void,
        failure: @escaping @MainActor (String) -> Void
    ) {
        starts += 1
        self.sink = sink
        self.failure = failure
    }

    func stop() {
        stops += 1
        sink = nil
    }

    func send(_ sample: MotionSample) {
        sink?(sample)
    }

    func fail(_ text: String) {
        failure?(text)
    }
}

@MainActor
struct FakeFeeds {
    let location = FakeLocationFeed()
    let heading = FakeHeadingFeed()
    let motion = FakeMotionFeed()

    func hub(core: any CoreService) -> SensorHub {
        SensorHub(core: core, location: location, heading: heading, motion: motion)
    }
}
