import Foundation
import SdrmmCore
import Security

nonisolated final class KeychainVault: SecretVault {
    static let defaultService = "dev.newspicel.sdrmm.vault"
    let service: String

    init(service: String = KeychainVault.defaultService) {
        self.service = service
    }

    func load(key: String) throws -> Data? {
        var query = base(key)
        query[kSecReturnData as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound {
            return nil
        }
        try check(status)
        guard let data = result as? Data else {
            throw VaultError.Corrupt
        }
        return data
    }

    func store(key: String, value: Data) throws {
        let update = [kSecValueData as String: value] as CFDictionary
        let status = SecItemUpdate(base(key) as CFDictionary, update)
        guard status == errSecItemNotFound else {
            return try check(status)
        }
        var item = base(key)
        item[kSecValueData as String] = value
        item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
        try check(SecItemAdd(item as CFDictionary, nil))
    }

    func delete(key: String) throws {
        let status = SecItemDelete(base(key) as CFDictionary)
        if status != errSecItemNotFound {
            try check(status)
        }
    }

    func keys() throws -> [String] {
        var query = serviceQuery()
        query[kSecReturnAttributes as String] = true
        query[kSecMatchLimit as String] = kSecMatchLimitAll
        var result: CFTypeRef?
        let status = SecItemCopyMatching(query as CFDictionary, &result)
        if status == errSecItemNotFound {
            return []
        }
        try check(status)
        guard let items = result as? [[String: Any]] else {
            throw VaultError.Corrupt
        }
        return items.compactMap { $0[kSecAttrAccount as String] as? String }.sorted()
    }

    private func serviceQuery() -> [String: Any] {
        [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrSynchronizable as String: false,
        ]
    }

    private func base(_ key: String) -> [String: Any] {
        var query = serviceQuery()
        query[kSecAttrAccount as String] = key
        return query
    }

    private func check(_ status: OSStatus) throws {
        guard status == errSecSuccess else {
            throw VaultError.Os(status: status)
        }
    }
}
