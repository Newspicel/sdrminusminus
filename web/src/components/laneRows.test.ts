import { describe, expect, it } from "vitest";
import type { Capabilities, DeviceSet, GainStage } from "../lib/types";
import {
  allLanesGain,
  laneGain,
  laneGains,
  laneLayout,
  meterStage,
  rxStages,
  spreadOf,
  steppedLanes,
  txStages,
} from "./laneRows";

const tuner: GainStage = { name: "tuner", kind: "tuner", range: { min: 0, max: 49.6, step: 0.1 } };
const amp: GainStage = { name: "amp", kind: "amp", range: { min: 0, max: 14 } };
const tx: GainStage = {
  name: "tx",
  kind: "tx",
  range: { min: -89, max: 0 },
  agc: { kind: "never" },
};

function capabilities(overrides: Partial<Capabilities> = {}): Capabilities {
  return {
    freq_ranges: [],
    sample_rates: [],
    gains: [tuner],
    antennas: [],
    bandwidths: [],
    extra: [],
    duplex: "rx_only",
    ...overrides,
  };
}

function deviceSet(caps: Capabilities, settings: DeviceSet["settings"] = {}): DeviceSet {
  return {
    id: 1,
    device: { driver: "virtual", key: "band", label: "Test" },
    capabilities: caps,
    settings,
    status: "running",
    channels: [],
    overruns: 0,
  };
}

describe("laneLayout", () => {
  it("draws one row for a single tuner", () => {
    expect(laneLayout(capabilities())).toEqual({
      lanes: 1,
      perLane: false,
      master: false,
      stepper: false,
    });
  });

  it("draws a master row above per-lane gains", () => {
    expect(laneLayout(capabilities({ rx_streams: 5, per_stream: { gain: true } }))).toEqual({
      lanes: 5,
      perLane: true,
      master: true,
      stepper: false,
    });
  });

  it("keeps a shared gain on one row even with several streams", () => {
    expect(laneLayout(capabilities({ rx_streams: 4 })).lanes).toBe(1);
  });

  it("adds a lane stepper when the radio offers a choice", () => {
    const layout = laneLayout(
      capabilities({ rx_streams: 1, rx_stream_choices: [1, 2], per_stream: { gain: true } }),
    );
    expect(layout).toEqual({ lanes: 1, perLane: false, master: true, stepper: true });
  });
});

describe("stages", () => {
  it("keeps transmit gain off the receive lanes", () => {
    const caps = capabilities({ gains: [amp, tuner, tx] });
    expect(rxStages(caps)).toEqual([amp, tuner]);
    expect(txStages(caps)).toEqual([tx]);
  });

  it("puts the meter on the stage the AGC drives, never on a switch", () => {
    expect(meterStage(capabilities({ gains: [amp, tuner] }))).toBe(tuner);
    expect(meterStage(capabilities({ gains: [amp] }))).toBeUndefined();
  });
});

describe("lane gains", () => {
  const caps = capabilities({ rx_streams: 2, per_stream: { gain: true } });

  it("reads each lane's own value and falls back to the stage floor", () => {
    const set = deviceSet(caps, {
      gains: [{ stage: "tuner", value_db: 20 }],
      streams: [{ stream: 1, gains: [{ stage: "tuner", value_db: 30 }] }],
    });
    expect(laneGains(set, tuner)).toEqual([20, 30]);
    expect(laneGains(deviceSet(caps), tuner)).toEqual([0, 0]);
  });

  it("writes one lane or every lane", () => {
    expect(laneGain(caps, 1, tuner, 12)).toEqual({
      streams: [{ stream: 1, gains: [{ stage: "tuner", value_db: 12 }] }],
    });
    expect(allLanesGain(caps, tuner, 12)).toEqual({
      streams: [0, 1].map((stream) => ({ stream, gains: [{ stage: "tuner", value_db: 12 }] })),
    });
    expect(allLanesGain(capabilities(), tuner, 12)).toEqual({
      gains: [{ stage: "tuner", value_db: 12 }],
    });
  });

  it("tells a uniform spread from a mixed one", () => {
    expect(spreadOf([20, 20])).toEqual({ uniform: true, mean: 20 });
    expect(spreadOf([10, 30])).toEqual({ uniform: false, mean: 20 });
    expect(spreadOf([])).toEqual({ uniform: true, mean: 0 });
  });
});

describe("steppedLanes", () => {
  it("walks the offered counts and stops at the ends", () => {
    expect(steppedLanes([1, 2, 4], 2, 1)).toBe(4);
    expect(steppedLanes([4, 1, 2], 4, 1)).toBe(4);
    expect(steppedLanes([1, 2, 4], 1, -1)).toBe(1);
    expect(steppedLanes([1, 2, 4], 3, 1)).toBe(2);
  });
});
