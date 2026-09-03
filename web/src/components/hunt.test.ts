import { describe, expect, it } from "vitest";
import type { DeviceSet, HuntStatus, PatchGraph } from "../lib/types";
import {
  bearing,
  DEFAULT_HUNT_SETTINGS,
  formatHuntDb,
  formatStrength,
  huntDeviceSet,
  huntRefusal,
  huntSettingsOf,
  liveHunt,
} from "./hunt";

const HUNT: HuntStatus = {
  settings: { freq_hz: 433_920_000, bw_hz: 12_500, interval_ms: 50 },
  level_db: -60,
  smooth_db: -61,
  floor_db: -90,
  best_db: -40,
  strength: 0.5,
  closing: false,
  readings: 10,
};

function deviceSet(over: Partial<DeviceSet> = {}): DeviceSet {
  return {
    id: 1,
    device: { driver: "virtual", key: "siggen", label: "Signal generator" },
    capabilities: {
      freq_ranges: [{ min: 1e6, max: 6e9 }],
      sample_rates: [2_048_000],
      gains: [],
      antennas: [],
      bandwidths: [],
      rx_streams: 1,
      tx_streams: 0,
      duplex: "rx_only",
    },
    settings: {},
    status: "running",
    channels: [],
    ...over,
  } as unknown as DeviceSet;
}

describe("liveHunt", () => {
  it("prefers the pushed reading over the one the state snapshot carried", () => {
    const set = deviceSet({ hunt: HUNT });
    expect(liveHunt(set, undefined)).toBe(HUNT);
    const fresher = { ...HUNT, readings: 99 };
    expect(liveHunt(set, fresher)?.readings).toBe(99);
  });

  it("reports nothing when the set is not hunting", () => {
    expect(liveHunt(deviceSet(), HUNT)).toBeNull();
    expect(liveHunt(null, HUNT)).toBeNull();
  });
});

describe("huntRefusal", () => {
  it("says why a hunt cannot start rather than failing at the server", () => {
    expect(huntRefusal(deviceSet(), 433_920_000)).toBeNull();
    expect(huntRefusal(deviceSet(), 9e9)).toMatch(/tuning range/);
    const scanning = deviceSet({
      scanner: {
        state: "scanning",
        settings: {} as never,
        targets: 1,
        first_hz: 1,
        last_hz: 1,
        current_hz: 1,
        sweeps: 0,
        hits: 0,
      } as never,
    });
    expect(huntRefusal(scanning, 433_920_000)).toMatch(/scanning/);
  });
});

describe("bearing", () => {
  it("waits for enough readings before pointing anywhere", () => {
    expect(bearing(null)).toBe("waiting");
    expect(bearing({ ...HUNT, readings: 1 })).toBe("waiting");
    expect(bearing({ ...HUNT, smooth_db: null })).toBe("waiting");
  });

  it("calls warmer, colder and on top of it", () => {
    expect(bearing({ ...HUNT, closing: true })).toBe("closing");
    expect(bearing({ ...HUNT, closing: false, strength: 0.3 })).toBe("leaving");
    expect(bearing({ ...HUNT, closing: false, strength: 0.95 })).toBe("steady");
  });
});

describe("formatting", () => {
  it("shows a dash rather than a number nobody measured", () => {
    expect(formatStrength(null)).toBe("—");
    expect(formatStrength({ ...HUNT, readings: 0 })).toBe("—");
    expect(formatStrength(HUNT)).toBe("50%");
    expect(formatHuntDb(null)).toBe("—");
    expect(formatHuntDb(Number.NaN)).toBe("—");
    expect(formatHuntDb(-61.25)).toBe("-61.3 dB");
  });
});

describe("a hunt node's own settings and radio", () => {
  const settings = { freq_hz: 145_500_000, bw_hz: 25_000, interval_ms: 50 };
  const graph: PatchGraph = {
    nodes: [
      {
        id: "dev",
        kind: "device",
        position: { x: 0, y: 0 },
        data: { device: { backend: "virtual", key: "siggen" } },
      },
      { id: "hunt", kind: "hunt", position: { x: 0, y: 0 }, data: { settings } },
      { id: "bare", kind: "hunt", position: { x: 0, y: 0 }, data: {} },
    ],
    edges: [{ from: { node: "hunt", port: "control" }, to: { node: "dev", port: "control" } }],
  };

  it("reads the frequency the node was left on, or the default", () => {
    expect(huntSettingsOf(graph, "hunt")).toEqual(settings);
    expect(huntSettingsOf(graph, "bare")).toEqual(DEFAULT_HUNT_SETTINGS);
    expect(huntSettingsOf(graph, "dev")).toEqual(DEFAULT_HUNT_SETTINGS);
  });

  it("finds the radio the hunt drives, and nothing for one wired to none", () => {
    const set = deviceSet();
    expect(huntDeviceSet(graph, [set], "hunt")).toBe(set);
    expect(huntDeviceSet(graph, [set], "bare")).toBeNull();
    expect(huntDeviceSet(graph, [], "hunt")).toBeNull();
  });
});
