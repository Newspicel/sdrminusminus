import SdrmmCore
import Security
import XCTest

@testable import SDRmm

@MainActor
final class KeychainVaultTests: XCTestCase {
    private let vault = KeychainVault(service: "dev.newspicel.sdrmm.vault.test.\(UUID().uuidString)")

    override func tearDownWithError() throws {
        for key in try vault.keys() {
            try vault.delete(key: key)
        }
        try super.tearDownWithError()
    }

    func testStoreLoadDelete() throws {
        try vault.store(key: "server/a", value: Data("secret".utf8))
        XCTAssertEqual(try vault.load(key: "server/a"), Data("secret".utf8))
        try vault.delete(key: "server/a")
        XCTAssertNil(try vault.load(key: "server/a"))
    }

    func testUpdateOverwrites() throws {
        try vault.store(key: "server/a", value: Data("one".utf8))
        try vault.store(key: "server/a", value: Data("two".utf8))
        XCTAssertEqual(try vault.load(key: "server/a"), Data("two".utf8))
        XCTAssertEqual(try vault.keys(), ["server/a"])
    }

    func testKeysListsOnlyThisService() throws {
        let other = KeychainVault(service: "dev.newspicel.sdrmm.vault.test.\(UUID().uuidString)")
        defer { try? other.delete(key: "server/other") }
        try other.store(key: "server/other", value: Data("x".utf8))
        try vault.store(key: "server/b", value: Data("b".utf8))
        try vault.store(key: "server/a", value: Data("a".utf8))
        XCTAssertEqual(try vault.keys(), ["server/a", "server/b"])
    }

    func testItemsStayOnThisDevice() throws {
        try vault.store(key: "server/a", value: Data("a".utf8))
        let query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: vault.service,
            kSecAttrAccount as String: "server/a",
            kSecAttrSynchronizable as String: kSecAttrSynchronizableAny,
            kSecReturnAttributes as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var result: CFTypeRef?
        XCTAssertEqual(SecItemCopyMatching(query as CFDictionary, &result), errSecSuccess)
        let attributes = try XCTUnwrap(result as? [String: Any])
        XCTAssertEqual(
            attributes[kSecAttrAccessible as String] as? String,
            kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly as String
        )
        XCTAssertNotEqual(attributes[kSecAttrSynchronizable as String] as? Bool, true)
    }

    func testMissingKeyIsNil() throws {
        XCTAssertNil(try vault.load(key: "server/none"))
        XCTAssertNoThrow(try vault.delete(key: "server/none"))
        XCTAssertEqual(try vault.keys(), [])
    }
}
