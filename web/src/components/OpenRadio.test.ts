import { describe, expect, it } from "vitest";
import type { DeviceInfo } from "../lib/types";
import {
  deviceId,
  filterRecordingDevices,
  groupDevices,
  isRecordingDevice,
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
    device("virtual", "file:/recordings/airband", "airband (recording)"),
    device("rtlsdr", "00000001", "RTL-SDR 00000001"),
  ];

  it("includes virtual devices in a development build", () => {
    expect(visibleDevices(devices, true).map(deviceId)).toEqual([
      "rtlsdr:00000001",
      "virtual:file:/recordings/airband",
      "virtual:array4",
      "virtual:siggen",
    ]);
  });

  it("omits synthetic devices from production but keeps recordings", () => {
    expect(visibleDevices(devices, false).map(deviceId)).toEqual([
      "rtlsdr:00000001",
      "virtual:file:/recordings/airband",
    ]);
  });
});

describe("groupDevices", () => {
  const devices = [
    device("rtlsdr", "00000001", "RTL-SDR 00000001"),
    device("virtual", "file:/recordings/airband", "airband (recording)"),
    device("virtual", "siggen", "Signal Generator"),
    device("virtual", "file:/recordings/weather", "weather (recording)"),
  ];

  it("keeps recordings out of the top-level device list", () => {
    const grouped = groupDevices(devices);

    expect(grouped.radios.map(deviceId)).toEqual(["rtlsdr:00000001", "virtual:siggen"]);
    expect(grouped.recordings.map(deviceId)).toEqual([
      "virtual:file:/recordings/airband",
      "virtual:file:/recordings/weather",
    ]);
  });

  it("recognizes only file-backed virtual devices as recordings", () => {
    expect(isRecordingDevice(device("rtlsdr", "00000001"))).toBe(false);
    expect(isRecordingDevice(device("virtual", "file:/recordings/airband"))).toBe(true);
    expect(isRecordingDevice(device("virtual", "siggen"))).toBe(false);
  });
});

describe("filterRecordingDevices", () => {
  const recordings = [
    device("virtual", "file:/recordings/Airband", "Airband morning (recording)"),
    device("virtual", "file:/recordings/weather", "Weather net (recording)"),
  ];

  it("matches labels without case sensitivity or surrounding whitespace", () => {
    expect(filterRecordingDevices(recordings, "  AIRBAND ").map(deviceId)).toEqual([
      "virtual:file:/recordings/Airband",
    ]);
  });

  it("returns every recording for an empty search", () => {
    expect(filterRecordingDevices(recordings, " ")).toEqual(recordings);
  });

  it("returns an empty list when no recording matches", () => {
    expect(filterRecordingDevices(recordings, "marine")).toEqual([]);
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
