import { describe, expect, it } from "vitest";
import type { DeviceInfo } from "../lib/types";
import {
  deviceId,
  NETWORK_BACKENDS,
  networkDeviceId,
  rankDevices,
  visibleDevices,
} from "./OpenRadio";

function device(driver: string, key: string, label = `${driver} ${key}`): DeviceInfo {
  return { driver, key, label };
}

describe("rankDevices", () => {
  /** A radio on the network is hardware like any other: someone who added one should not have to
   * read past the signal generator to find it again. */
  it("puts every real radio above the virtual devices", () => {
    const ranked = rankDevices([
      device("virtual", "siggen", "Signal Generator"),
      device("rtlsdr", "00000001", "RTL-SDR 00000001"),
      device("rtltcp", "10.0.0.5:1234", "rtl_tcp 10.0.0.5:1234"),
    ]);
    expect(
      ranked
        .map((d) => d.driver)
        .slice(0, 2)
        .toSorted(),
    ).toEqual(["rtlsdr", "rtltcp"]);
    expect(ranked.at(-1)?.driver).toBe("virtual");
  });
});

describe("visibleDevices", () => {
  const devices = [
    device("virtual", "siggen", "Signal Generator"),
    device("virtual", "array4", "Coherent Array"),
    device("rtlsdr", "00000001", "RTL-SDR 00000001"),
  ];

  it("includes virtual devices in a development build", () => {
    expect(visibleDevices(devices, true).map(deviceId)).toEqual([
      "rtlsdr:00000001",
      "virtual:array4",
      "virtual:siggen",
    ]);
  });

  it("omits virtual devices from a production build", () => {
    expect(visibleDevices(devices, false).map(deviceId)).toEqual(["rtlsdr:00000001"]);
  });
});

describe("networkDeviceId", () => {
  it("composes the id the open endpoint takes", () => {
    expect(networkDeviceId("rtltcp", "10.0.0.5:1234")).toBe("rtltcp:10.0.0.5:1234");
    expect(networkDeviceId("spyserver", "spy.local")).toBe("spyserver:spy.local");
  });

  /** The server defaults the port and lower-cases the host; guessing either here would be a second
   * address parser, and the patch would store a key the probe never reports. */
  it("passes the address through untouched", () => {
    expect(networkDeviceId("rtltcp", "  Radio.Local  ")).toBe("rtltcp:Radio.Local");
    expect(networkDeviceId("rtltcp", "[2001:db8::1]:1234")).toBe("rtltcp:[2001:db8::1]:1234");
  });

  it("strips a scheme someone pasted, but never an IPv6 literal's colons", () => {
    expect(networkDeviceId("rtltcp", "rtl_tcp://10.0.0.5:1234")).toBe("rtltcp:10.0.0.5:1234");
    expect(networkDeviceId("spyserver", "sdr://spy.local:5555")).toBe("spyserver:spy.local:5555");
    expect(networkDeviceId("rtltcp", "::1")).toBe("rtltcp:::1");
  });

  it("has nothing to send for an address that is not one", () => {
    for (const address of ["", "   ", "10.0.0.5 1234", "rtl_tcp://"]) {
      expect(networkDeviceId("rtltcp", address)).toBeNull();
    }
  });

  /** The composed id has to survive the split `DeviceRegistry::open` does on the *first* colon,
   * which is what makes an IPv6 endpoint reach the driver intact. */
  it("round-trips through the id a probed device would report", () => {
    const id = networkDeviceId("rtltcp", "[::1]:1234");
    expect(id).not.toBeNull();
    const at = (id ?? "").indexOf(":");
    expect((id ?? "").slice(0, at)).toBe("rtltcp");
    expect((id ?? "").slice(at + 1)).toBe("[::1]:1234");
    expect(deviceId(device("rtltcp", "[::1]:1234"))).toBe(id);
  });
});

describe("NETWORK_BACKENDS", () => {
  /** The drivers the server registers. A rename on either side has to break something. */
  it("names the two protocols and shows each one's default port", () => {
    expect(NETWORK_BACKENDS.map((b) => b.driver)).toEqual(["rtltcp", "spyserver"]);
    expect(NETWORK_BACKENDS.map((b) => b.placeholder.split(":").pop())).toEqual(["1234", "5555"]);
  });
});
