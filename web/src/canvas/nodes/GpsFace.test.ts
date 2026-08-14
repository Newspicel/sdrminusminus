import { describe, expect, it } from "vitest";
import { validGpsdAddress } from "./GpsFace";

describe("validGpsdAddress", () => {
  it("accepts host and bracketed IPv6 endpoints", () => {
    expect(validGpsdAddress("127.0.0.1:2947")).toBe(true);
    expect(validGpsdAddress("gps.local:2947")).toBe(true);
    expect(validGpsdAddress("[::1]:2947")).toBe(true);
  });

  it("rejects missing hosts, ports, and malformed endpoints", () => {
    for (const address of ["", "localhost", "localhost:0", "bad host:2947", "[::1:2947"]) {
      expect(validGpsdAddress(address), address).toBe(false);
    }
  });
});
