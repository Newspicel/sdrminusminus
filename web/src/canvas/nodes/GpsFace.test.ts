import { describe, expect, it } from "vitest";
import {
  filterNmeaDevices,
  nmeaDetail,
  nmeaSource,
  nmeaSuggestion,
  validGpsdAddress,
} from "./gpsSource";

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

  it("says nothing more about a port that reports no identity", () => {
    expect(nmeaSuggestion({ path: "/dev/ttyS0" })).toEqual({ value: "/dev/ttyS0" });
  });
});

describe("the receiver list", () => {
  const devices = [
    { path: "/dev/cu.usbmodem11401", product: "GNSS receiver", manufacturer: "u-blox" },
    { path: "/dev/ttyS0" },
  ];

  it("names a receiver by what it reports, and says nothing for a bare port", () => {
    expect(nmeaDetail(devices[0]!)).toBe("GNSS receiver");
    expect(nmeaDetail({ ...devices[0]!, serial: "GPS-1" })).toBe("GNSS receiver · GPS-1");
    expect(nmeaDetail(devices[1]!)).toBe("");
  });

  it("filters on the path and on what the receiver calls itself", () => {
    expect(filterNmeaDevices(devices, " USBMODEM ").map((d) => d.path)).toEqual([
      "/dev/cu.usbmodem11401",
    ]);
    expect(filterNmeaDevices(devices, "u-blox").map((d) => d.path)).toEqual([
      "/dev/cu.usbmodem11401",
    ]);
    expect(filterNmeaDevices(devices, " ")).toEqual(devices);
    expect(filterNmeaDevices(devices, "garmin")).toEqual([]);
  });

  it("reads a chosen port at the rate a receiver ships with", () => {
    expect(nmeaSource("/dev/ttyS0")).toEqual({
      type: "nmea",
      device: "/dev/ttyS0",
      baud: 9_600,
      update_interval_ms: 1_000,
    });
  });
});
