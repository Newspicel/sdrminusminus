import { describe, expect, it } from "vitest";
import type { DeviceInfo } from "../lib/types";
import {
  deviceId,
  groupDevices,
  NETWORK_BACKENDS,
  networkDeviceId,
  rankDevices,
  sourceTabs,
  unclaimedDevices,
  visibleDevices,
} from "./devices";

function device(driver: string, key: string, label = `${driver} ${key}`): DeviceInfo {
  return { driver, key, label };
}

describe("rankDevices", () => {
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
    device("recording", "airband", "airband"),
    device("siggen", "signal_gen-a1b2", "Signal generator"),
    device("array", "array-9f2c", "Array"),
    device("rtlsdr", "00000001", "RTL-SDR 00000001"),
  ];

  it("leaves every radio a node of its own opens out of the picker", () => {
    expect(visibleDevices(devices, true).map(deviceId)).toEqual([
      "rtlsdr:00000001",
      "virtual:array4",
      "virtual:siggen",
    ]);
  });

  it("omits synthetic radios from a production build", () => {
    expect(visibleDevices(devices, false).map(deviceId)).toEqual(["rtlsdr:00000001"]);
  });
});

describe("unclaimedDevices", () => {
  const devices = [
    device("rtlsdr", "00000001", "RTL-SDR 00000001"),
    device("rtlsdr", "00000002", "RTL-SDR 00000002"),
    device("virtual", "siggen", "Signal Generator"),
  ];

  it("drops the radios another node already names", () => {
    expect(
      unclaimedDevices(devices, [{ backend: "rtlsdr", key: "00000001" }]).map(deviceId),
    ).toEqual(["rtlsdr:00000002", "virtual:siggen"]);
    expect(
      unclaimedDevices(
        [{ driver: "rtlsdr", key: "0@rx", label: "RTL-SDR", serial: "0" }],
        [{ backend: "rtlsdr", serial: "0", key: "0@rx" }],
      ),
    ).toEqual([]);
  });

  it("matches a keyed reference and a bare backend alike", () => {
    expect(
      unclaimedDevices(devices, [{ backend: "virtual", key: "siggen" }]).map(deviceId),
    ).toEqual(["rtlsdr:00000001", "rtlsdr:00000002"]);
    expect(unclaimedDevices(devices, [{ backend: "virtual" }]).map(deviceId)).toEqual([
      "rtlsdr:00000001",
      "rtlsdr:00000002",
    ]);
  });

  it("offers everything when no node holds a radio", () => {
    expect(unclaimedDevices(devices, [])).toEqual(devices);
  });
});

describe("groupDevices", () => {
  const devices = [
    device("rtlsdr", "00000001", "RTL-SDR 00000001"),
    device("virtual", "siggen", "Signal Generator"),
    device("virtual", "array4", "Coherent Array"),
  ];

  it("keeps the synthetic radios out of the top-level device list", () => {
    const grouped = groupDevices(devices);

    expect(grouped.radios.map(deviceId)).toEqual(["rtlsdr:00000001"]);
    expect(grouped.virtual.map(deviceId)).toEqual(["virtual:siggen", "virtual:array4"]);
  });
});

describe("sourceTabs", () => {
  const groups = groupDevices([
    device("rtlsdr", "00000001", "RTL-SDR 00000001"),
    device("virtual", "siggen", "Signal Generator"),
  ]);

  it("counts what each tab holds and explains each one on hover", () => {
    const tabs = sourceTabs(groups);
    expect(tabs.map((tab) => [tab.value, tab.label])).toEqual([
      ["radios", "Radios"],
      ["network", "Network"],
      ["virtual", "Virtual (1)"],
    ]);
    expect(tabs.every((tab) => (tab.title ?? "") !== "")).toBe(true);
  });

  it("offers no virtual tab in a build without virtual radios", () => {
    const tabs = sourceTabs({ ...groups, virtual: [] });
    expect(tabs.map((tab) => tab.label)).toEqual(["Radios", "Network"]);
  });
});

describe("networkDeviceId", () => {
  it("composes the id the open endpoint takes", () => {
    expect(networkDeviceId("rtltcp", "10.0.0.5:1234")).toBe("rtltcp:10.0.0.5:1234");
    expect(networkDeviceId("spyserver", "spy.local")).toBe("spyserver:spy.local");
  });

  it("passes the address through untouched", () => {
    expect(networkDeviceId("rtltcp", "  Radio.Local  ")).toBe("rtltcp:Radio.Local");
    expect(networkDeviceId("rtltcp", "[2001:db8::1]:1234")).toBe("rtltcp:[2001:db8::1]:1234");
  });

  it("strips a scheme someone pasted, but never an IPv6 literal's colons", () => {
    expect(networkDeviceId("rtltcp", "rtl_tcp://10.0.0.5:1234")).toBe("rtltcp:10.0.0.5:1234");
    expect(networkDeviceId("spyserver", "sdr://spy.local:5555")).toBe("spyserver:spy.local:5555");
    expect(networkDeviceId("sdrconnect", "ws://rsp.local:5454")).toBe("sdrconnect:rsp.local:5454");
    expect(networkDeviceId("rtltcp", "::1")).toBe("rtltcp:::1");
  });

  it("has nothing to send for an address that is not one", () => {
    for (const address of ["", "   ", "10.0.0.5 1234", "rtl_tcp://"]) {
      expect(networkDeviceId("rtltcp", address)).toBeNull();
    }
  });

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
  it("names each protocol and shows its default port", () => {
    expect(NETWORK_BACKENDS.map((b) => b.driver)).toEqual([
      "rtltcp",
      "spyserver",
      "sdrconnect",
      "ad936x",
    ]);
    expect(NETWORK_BACKENDS.map((b) => b.placeholder.split(":").pop())).toEqual([
      "1234",
      "5555",
      "5454",
      "30431",
    ]);
  });
});
