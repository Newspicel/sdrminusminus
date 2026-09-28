import os

nonisolated enum Log {
    static let core = Logger(subsystem: "dev.newspicel.sdrmm", category: "core")
    static let sensors = Logger(subsystem: "dev.newspicel.sdrmm", category: "sensors")
    static let audio = Logger(subsystem: "dev.newspicel.sdrmm", category: "audio")
    static let nav = Logger(subsystem: "dev.newspicel.sdrmm", category: "nav")
    static let carplay = Logger(subsystem: "dev.newspicel.sdrmm", category: "carplay")
    static let pair = Logger(subsystem: "dev.newspicel.sdrmm", category: "pair")
}

nonisolated struct NotBuilt: Error, Equatable {
    let feature: String
}
