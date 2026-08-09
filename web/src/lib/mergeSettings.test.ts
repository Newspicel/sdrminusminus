// Mirrors the `DeviceSettings::merge_from` tests (crates/wire/src/device.rs): the optimistic
// cache must merge exactly like the server, or accumulated edits drift from the applied state.
import { describe, expect, it } from "vitest";
import type { DeviceSettings, StateSnapshot } from "./types";
import { mergeSettings, patchTargetExists } from "./useDevicePatch";

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

// Guards the debounce-flush-after-close path: a patch for a deleted set must be dropped, not
// sent and then surfaced as a stale "Rejected" banner on the next device.
describe("patchTargetExists", () => {
  it("is true only for a set present in the snapshot", () => {
    expect(patchTargetExists(snapshot(0, 3), 3)).toBe(true);
    expect(patchTargetExists(snapshot(0, 3), 1)).toBe(false);
  });

  it("is false with no snapshot at all", () => {
    expect(patchTargetExists(undefined, 0)).toBe(false);
  });
});
