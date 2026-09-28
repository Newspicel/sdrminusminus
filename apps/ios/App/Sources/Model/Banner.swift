import SdrmmCore

struct Banner: Identifiable, Equatable {
    let id: Int
    let level: NoticeLevel
    let text: String
    let detail: String?

    var seconds: Double {
        switch level {
        case .info: 4
        case .warn: 6
        case .error: 8
        }
    }
}
