import Foundation

@testable import SDRmm

@MainActor
final class TestClock: NavClock {
    private struct Sleeper {
        let id: UUID
        let until: Date
        let continuation: CheckedContinuation<Void, Error>
    }

    private(set) var current: Date
    private var sleepers: [Sleeper] = []

    init(start: Date = Date(timeIntervalSince1970: 1_800_000_000)) {
        current = start
    }

    func now() -> Date {
        current
    }

    func sleep(until date: Date) async throws {
        guard date > current else {
            return
        }
        let id = UUID()
        try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                sleepers.append(Sleeper(id: id, until: date, continuation: continuation))
            }
        } onCancel: {
            Task { @MainActor in self.cancel(id) }
        }
    }

    func advance(by seconds: TimeInterval) {
        advance(to: current.addingTimeInterval(seconds))
    }

    func advance(to date: Date) {
        current = date
        let due = sleepers.filter { $0.until <= date }
        sleepers.removeAll { $0.until <= date }
        for sleeper in due {
            sleeper.continuation.resume()
        }
    }

    private func cancel(_ id: UUID) {
        guard let index = sleepers.firstIndex(where: { $0.id == id }) else {
            return
        }
        sleepers.remove(at: index).continuation.resume(throwing: CancellationError())
    }
}
