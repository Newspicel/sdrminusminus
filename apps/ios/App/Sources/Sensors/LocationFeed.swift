import CoreLocation
import os

final class LocationFeed: NSObject, LocationFeeding, CLLocationManagerDelegate {
    private static let restartDelay: Duration = .seconds(2)
    private static let maxRestarts = 5
    private let manager = CLLocationManager()
    private var sink: (@MainActor (LocationFeedEvent) -> Void)?
    private var statusSink: (@MainActor (LocationFeedEvent) -> Void)?
    private var loop: Task<Void, Never>?
    private var session: CLServiceSession?
    private var alwaysSession: CLServiceSession?
    private var preciseSession: CLServiceSession?
    private var background: CLBackgroundActivitySession?

    override init() {
        super.init()
        manager.delegate = self
    }

    func observeAuthorization(_ sink: @escaping @MainActor (LocationFeedEvent) -> Void) {
        statusSink = sink
        reportAuthorization()
    }

    func start(profile: SensorProfile, sink: @escaping @MainActor (LocationFeedEvent) -> Void) {
        halt()
        self.sink = sink
        session = CLServiceSession(authorization: .whenInUse)
        background = CLBackgroundActivitySession()
        reportAuthorization()
        let configuration: CLLocationUpdate.LiveConfiguration =
            profile == .drive ? .automotiveNavigation : .otherNavigation
        loop = Task { [weak self] in
            await self?.run(configuration)
        }
    }

    func stop() {
        halt()
        alwaysSession = nil
        preciseSession = nil
    }

    private func halt() {
        loop?.cancel()
        loop = nil
        background?.invalidate()
        background = nil
        session = nil
        sink = nil
    }

    func requestAlways() {
        alwaysSession = CLServiceSession(authorization: .always)
    }

    func requestPrecise() {
        preciseSession = CLServiceSession(
            authorization: .whenInUse,
            fullAccuracyPurposeKey: "DirectionFinding"
        )
    }

    func locationManagerDidChangeAuthorization(_ manager: CLLocationManager) {
        reportAuthorization()
    }

    private func run(_ configuration: CLLocationUpdate.LiveConfiguration) async {
        var restarts = 0
        while !Task.isCancelled {
            do {
                for try await update in CLLocationUpdate.liveUpdates(configuration) {
                    deliver(update)
                }
                return
            } catch {
                restarts += 1
                Log.sensors.error("location updates failed: \(error.localizedDescription, privacy: .public)")
                emit(.failed(restarts > Self.maxRestarts ? "Location stopped" : "Location error"))
                guard restarts <= Self.maxRestarts else {
                    return
                }
                do {
                    try await Task.sleep(for: Self.restartDelay)
                } catch {
                    return
                }
            }
        }
    }

    private func deliver(_ update: CLLocationUpdate) {
        if update.authorizationDenied || update.authorizationDeniedGlobally {
            emit(.access(.denied))
        } else if update.authorizationRestricted {
            emit(.access(.restricted))
        }
        if update.accuracyLimited {
            emit(.precise(false))
        }
        if update.locationUnavailable {
            emit(.unavailable)
        }
        if let location = update.location {
            emit(.fix(location))
        }
    }

    private func emit(_ event: LocationFeedEvent) {
        sink?(event)
    }

    private func reportAuthorization() {
        let access = Self.access(manager.authorizationStatus)
        let precise = manager.accuracyAuthorization == .fullAccuracy
        for target in [statusSink, sink].compactMap({ $0 }) {
            target(.access(access))
            target(.precise(precise))
        }
    }

    private static func access(_ status: CLAuthorizationStatus) -> LocationAccess {
        switch status {
        case .notDetermined: .unknown
        case .restricted: .restricted
        case .denied: .denied
        case .authorizedAlways: .always
        case .authorizedWhenInUse: .whenInUse
        @unknown default: .unknown
        }
    }
}
