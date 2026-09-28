import Network
import XCTest

@testable import SDRmm

final class BonjourTxtTests: XCTestCase {
    func testDecodesKeysAndValues() {
        var record = NWTXTRecord(["id": "s1", "n": "Lab Pi", "fp": "abc"])
        record.setEntry(.empty, for: "flag")
        XCTAssertEqual(BonjourTxt.decode(record), ["id": "s1", "n": "Lab Pi", "fp": "abc", "flag": ""])
    }

    func testSkipsNonUtf8Values() {
        var record = NWTXTRecord(["id": "s1"])
        record.setEntry(.data(Data([0xFF, 0xFE, 0xFD])), for: "bad")
        XCTAssertEqual(BonjourTxt.decode(record), ["id": "s1"])
    }

    func testHostPortFormatting() throws {
        let port = try XCTUnwrap(NWEndpoint.Port(rawValue: 8443))
        let v4 = try XCTUnwrap(IPv4Address("10.0.0.2"))
        let global = try XCTUnwrap(IPv6Address("2001:db8::1"))
        let local = try XCTUnwrap(IPv6Address("fe80::1"))
        XCTAssertEqual(BonjourTxt.hostPort(.hostPort(host: .ipv4(v4), port: port)), "10.0.0.2:8443")
        XCTAssertEqual(BonjourTxt.hostPort(.hostPort(host: .ipv6(global), port: port)), "[2001:db8::1]:8443")
        XCTAssertNil(BonjourTxt.hostPort(.hostPort(host: .ipv6(local), port: port)))
        XCTAssertEqual(
            BonjourTxt.hostPort(.hostPort(host: .name("pi.local", nil), port: port)),
            "pi.local:8443"
        )
        XCTAssertNil(BonjourTxt.hostPort(nil))
    }
}
