import Observation

final class ObservationLoop {
    private let body: @MainActor () -> Void
    private var active = true

    init(_ body: @escaping @MainActor () -> Void) {
        self.body = body
        arm()
    }

    func cancel() {
        active = false
    }

    private func arm() {
        guard active else {
            return
        }
        withObservationTracking {
            body()
        } onChange: { [weak self] in
            Task { @MainActor in
                self?.arm()
            }
        }
    }
}
