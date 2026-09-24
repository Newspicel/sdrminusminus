import { describe, expect, it } from "vitest";
import type { ChannelInfo, DeviceSet, HuntStatus, PatchGraph } from "../lib/types";
import {
  bearing,
  formatHuntDb,
  formatStrength,
  huntedHz,
  huntRefusal,
  huntTarget,
  liveHunt,
} from "./hunt";

const HUNT: HuntStatus = {
  settings: { channel: 9, interval_ms: 50 },
  freq_hz: 433_920_000,
  bw_hz: 12_500,
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
    const set = deviceSet({ hunts: [HUNT] });
    expect(liveHunt(set, 9, undefined)).toBe(HUNT);
    const fresher = { ...HUNT, readings: 99 };
    expect(liveHunt(set, 9, fresher)?.readings).toBe(99);
  });

  it("reports nothing when the decoder is not hunted", () => {
    expect(liveHunt(deviceSet({ hunts: [HUNT] }), 3, HUNT)).toBeNull();
    expect(liveHunt(deviceSet(), 9, HUNT)).toBeNull();
    expect(liveHunt(null, 9, HUNT)).toBeNull();
  });
});

const CHANNEL: ChannelInfo = {
  id: 9,
  stream: 0,
  settings: { frequency_hz: 433_920_000, params: { type: "nfm", settings: {} } as never },
};

describe("huntRefusal", () => {
  it("says why a hunt cannot start rather than failing at the server", () => {
    expect(huntRefusal(null)).toBeNull();
    expect(huntRefusal({ set: deviceSet(), channel: CHANNEL })).toBeNull();
    expect(huntRefusal({ set: deviceSet(), channel: { ...CHANNEL, out_of_band: true } })).toMatch(
      /tuned away/,
    );
    const scan = {
      state: "scanning",
      settings: { channel: CHANNEL.id } as never,
      targets: 1,
      first_hz: 1,
      last_hz: 1,
      current_hz: 1,
      sweeps: 0,
      hits: 0,
    } as never;
    const scanning = deviceSet({ scanners: [scan] });
    expect(huntRefusal({ set: scanning, channel: CHANNEL })).toMatch(/scanning/);
    expect(huntRefusal({ set: scanning, channel: { ...CHANNEL, id: 2 } })).toBeNull();
  });
});

describe("huntedHz", () => {
  it("shows the decoder's frequency until a reading says otherwise", () => {
    expect(huntedHz(null, null)).toBeNull();
    expect(huntedHz(null, CHANNEL)).toBe(433_920_000);
    expect(huntedHz({ ...HUNT, freq_hz: 145_500_000 }, CHANNEL)).toBe(145_500_000);
    expect(huntedHz({ ...HUNT, freq_hz: 0 }, CHANNEL)).toBe(433_920_000);
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
    expect(formatStrength(null)).toBe("-");
    expect(formatStrength({ ...HUNT, readings: 0 })).toBe("-");
    expect(formatStrength(HUNT)).toBe("50%");
    expect(formatHuntDb(null)).toBe("-");
    expect(formatHuntDb(Number.NaN)).toBe("-");
    expect(formatHuntDb(-61.25)).toBe("-61.3 dB");
  });
});

describe("the decoder a hunt drives", () => {
  const graph: PatchGraph = {
    nodes: [
      {
        id: "dev",
        kind: "device",
        position: { x: 0, y: 0 },
        data: { device: { backend: "virtual", key: "siggen" } },
      },
      { id: "nfm", kind: "channel", position: { x: 0, y: 0 }, data: { channel_type: "nfm" } },
      { id: "hunt", kind: "hunt", position: { x: 0, y: 0 }, data: { clicks: false } },
      { id: "bare", kind: "hunt", position: { x: 0, y: 0 }, data: {} },
    ],
    edges: [
      { from: { node: "dev", port: "iq" }, to: { node: "nfm", port: "iq" } },
      { from: { node: "hunt", port: "control" }, to: { node: "nfm", port: "control" } },
    ],
  };

  it("finds the decoder and its radio, and nothing for a hunt wired to none", () => {
    const set = deviceSet({ channels: [CHANNEL] });
    expect(huntTarget(graph, [set], "hunt")).toEqual({ set, channel: CHANNEL });
    expect(huntTarget(graph, [set], "bare")).toBeNull();
    expect(huntTarget(graph, [], "hunt")).toBeNull();
  });

  it("waits for the decoder to be open before offering a target", () => {
    expect(huntTarget(graph, [deviceSet()], "hunt")).toBeNull();
  });
});
