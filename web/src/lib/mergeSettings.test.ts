import { describe, expect, it } from "vitest";
import type { DeviceSettings, StateSnapshot } from "./types";
import { forStream, mergeSettings, patchTargetExists } from "./useDevicePatch";

function gain(stage: string, value_db: number) {
  return { stage, value_db };
}

function extra(name: string, value: unknown) {
  return { name, value };
}

describe("mergeSettings", () => {
  it("patches one gain stage and appends new ones", () => {
    const current: DeviceSettings = { gains: [gain("LNA", 16.0), gain("VGA", 20.0)] };
    const next = mergeSettings(current, { gains: [gain("VGA", 30.0), gain("AMP", 14.0)] });
    expect(next.gains).toEqual([gain("LNA", 16.0), gain("VGA", 30.0), gain("AMP", 14.0)]);
  });

  it("patches extra by name", () => {
    const current: DeviceSettings = { extra: [extra("bias_t", false), extra("agc", true)] };
    const next = mergeSettings(current, {
      extra: [extra("bias_t", true), extra("offset_tuning", true)],
    });
    expect(next.extra).toEqual([
      extra("bias_t", true),
      extra("agc", true),
      extra("offset_tuning", true),
    ]);
  });

  it("overlays bandwidth and leaves absent fields", () => {
    const current: DeviceSettings = { center_hz: 100_000_000, bandwidth: 2_500_000 };
    const next = mergeSettings(current, { bandwidth: 1_750_000 });
    expect(next.center_hz).toBe(100_000_000);
    expect(next.bandwidth).toBe(1_750_000);
    expect(mergeSettings(next, {}).bandwidth).toBe(1_750_000);
  });

  it("merges stream overrides by index, and each entry's gains by stage", () => {
    const current: DeviceSettings = {
      streams: [
        { stream: 0, center_hz: 100_000_000, gains: [gain("LNA", 16.0)] },
        { stream: 1, center_hz: 433_920_000 },
      ],
    };
    const next = mergeSettings(current, {
      streams: [
        { stream: 0, gains: [gain("LNA", 24.0), gain("VGA", 10.0)] },
        { stream: 2, antenna: "RX2" },
      ],
    });
    expect(next.streams).toEqual([
      { stream: 0, center_hz: 100_000_000, gains: [gain("LNA", 24.0), gain("VGA", 10.0)] },
      { stream: 1, center_hz: 433_920_000 },
      { stream: 2, antenna: "RX2" },
    ]);
  });

  it("keeps stream overrides across a radio-wide retune", () => {
    const current: DeviceSettings = {
      center_hz: 100_000_000,
      streams: [{ stream: 1, center_hz: 433_920_000 }],
    };
    const next = mergeSettings(current, { center_hz: 145_500_000 });
    expect(next.center_hz).toBe(145_500_000);
    expect(next.streams).toEqual([{ stream: 1, center_hz: 433_920_000 }]);
  });
});

describe("forStream", () => {
  const settings: DeviceSettings = {
    center_hz: 100_000_000,
    antenna: "RX",
    gains: [gain("LNA", 16.0)],
    streams: [{ stream: 1, center_hz: 433_920_000, antenna: "RX2", gains: [gain("LNA", 24.0)] }],
  };

  it("applies only the scoped settings of a lane's override", () => {
    const lane = forStream(settings, 1, { tuning: true });
    expect(lane.center_hz).toBe(433_920_000);
    expect(lane.antenna).toBe("RX");
    expect(lane.gains).toEqual([gain("LNA", 16.0)]);
    expect(lane.streams).toBeUndefined();

    const gainOnly = forStream(settings, 1, { gain: true, antenna: true });
    expect(gainOnly.center_hz).toBe(100_000_000);
    expect(gainOnly.antenna).toBe("RX2");
    expect(gainOnly.gains).toEqual([gain("LNA", 24.0)]);
  });

  it("resolves a lane without an override to the radio-wide settings", () => {
    const lane = forStream(settings, 0, { tuning: true, gain: true, antenna: true });
    expect(lane.center_hz).toBe(100_000_000);
    expect(lane.antenna).toBe("RX");
    expect(lane.gains).toEqual([gain("LNA", 16.0)]);
  });
});

function snapshot(...ids: number[]): StateSnapshot {
  return {
    revision: 1,
    device_sets: ids.map((id) => ({
      id,
      device: { driver: "virtual", key: "0", label: "Virtual" },
      settings: {},
      capabilities: { antennas: [], bandwidths: [], freq_ranges: [], gains: [], sample_rates: [] },
      status: "running",
      channels: [],
    })),
  };
}

describe("patchTargetExists", () => {
  it("is true only for a set present in the snapshot", () => {
    expect(patchTargetExists(snapshot(0, 3), 3)).toBe(true);
    expect(patchTargetExists(snapshot(0, 3), 1)).toBe(false);
  });

  it("is false with no snapshot at all", () => {
    expect(patchTargetExists(undefined, 0)).toBe(false);
  });
});
