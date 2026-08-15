import { describe, expect, it } from "vitest";
import { nmeaSuggestion, validGpsdAddress } from "./GpsFace";

describe("validGpsdAddress", () => {
  it("accepts host and bracketed IPv6 endpoints", () => {
    expect(validGpsdAddress("127.0.0.1:2947")).toBe(true);
    expect(validGpsdAddress("gps.local:2947")).toBe(true);
    expect(validGpsdAddress("[::1]:2947")).toBe(true);
  });

  it("rejects missing hosts, ports, and malformed endpoints", () => {
    for (const address of [
      "",
      "localhost",
      "localhost:0",
      "bad host:2947",
      "[::1:2947",
      "[::::]:2947",
      "[1:2:3:4:5:6:7:8:9]:2947",
    ]) {
      expect(validGpsdAddress(address), address).toBe(false);
    }
  });
});

describe("nmeaSuggestion", () => {
  it("names the receiver behind the path", () => {
    expect(
      nmeaSuggestion({
        path: "/dev/cu.usbmodem11401",
        product: "GNSS receiver",
        manufacturer: "u-blox",
        serial: "GPS-1",
      }),
    ).toEqual({ value: "/dev/cu.usbmodem11401", detail: "GNSS receiver · GPS-1" });
  });

  /** The path is already the item's own line; repeating it as the detail was what made the list
   * read as two copies of one entry. */
  it("says nothing more about a port that reports no identity", () => {
    expect(nmeaSuggestion({ path: "/dev/ttyS0" })).toEqual({ value: "/dev/ttyS0" });
  });
});
