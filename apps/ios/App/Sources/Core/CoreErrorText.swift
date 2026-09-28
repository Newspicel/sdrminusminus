import Foundation
import SdrmmCore

nonisolated enum CoreErrorText {
    static func short(_ error: Error) -> String {
        if let missing = error as? NotBuilt {
            return "\(missing.feature) not built yet"
        }
        guard let error = error as? CoreError else {
            return "Error"
        }
        switch error {
        case .InvalidLink: return "Bad QR"
        case .WrongCode: return "Wrong code"
        case .CodeExpired: return "Code expired"
        case .ProtocolMismatch(let server, let app): return server < app ? "Server too old" : "App too old"
        case .Unreachable: return "No answer"
        case .LocalNetworkBlocked: return "Local network off"
        case .KeyMismatch: return "Key mismatch"
        case .Revoked: return "Phone removed"
        case .Vault: return "Keychain error"
        case .Server(let status, _): return "Server error \(status)"
        case .NotConnected: return "Offline"
        case .NoMission: return "No mission"
        case .Refused(let message): return message
        case .Internal: return "Core error"
        }
    }

    static func detail(_ error: Error) -> String {
        if error is NotBuilt {
            return short(error)
        }
        guard let error = error as? CoreError else {
            return error.localizedDescription
        }
        switch error {
        case .InvalidLink(let reason): return "Bad QR: \(reason)"
        case .ProtocolMismatch(let server, let app): return "Server protocol \(server), app protocol \(app)"
        case .Unreachable(let hosts):
            let from = hosts.isEmpty ? "" : " from \(hosts.joined(separator: ", "))"
            return "No answer\(from). Check Settings > Privacy > Local Network"
        case .LocalNetworkBlocked: return "Allow Local Network in Settings > Privacy"
        case .Vault(let status): return "Keychain status \(status)"
        case .Server(let status, let message): return "Server error \(status): \(message)"
        case .Refused(let message), .Internal(let message): return message
        case .WrongCode, .CodeExpired, .KeyMismatch, .Revoked, .NotConnected, .NoMission:
            return short(error)
        }
    }
}
