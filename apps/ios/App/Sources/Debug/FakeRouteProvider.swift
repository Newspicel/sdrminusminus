#if DEBUG
    import SdrmmCore

    final class FakeRouteProvider: RouteProviding {
        private let plans: [RoutePlan]
        private var failNext: RouteError?
        private(set) var requests: [(LatLon, LatLon)] = []
        private(set) var cancels = 0

        init(plans: [RoutePlan]) {
            self.plans = plans
        }

        func routes(from: LatLon, to: LatLon, alternatives: Bool) async throws(RouteError) -> [RoutePlan] {
            requests.append((from, to))
            if let error = failNext {
                failNext = nil
                throw error
            }
            return alternatives ? plans : Array(plans.prefix(1))
        }

        func cancel() {
            cancels += 1
        }

        func fail(next: RouteError) {
            failNext = next
        }
    }
#endif
