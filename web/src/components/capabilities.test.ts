import { describe, expect, it } from "vitest";
import type { GainStage } from "../lib/types";
import {
  agcState,
  automaticGainIsOn,
  dcBlockOn,
  filterHz,
  fitsSlider,
  formatGain,
  gainLabel,
  gainUnit,
  hasDcArtifact,
  hasFilter,
  isSwitch,
  settingIndex,
  snapToRanges,
  snapToStage,
  spanOf,
  stageSettings,
} from "./capabilities";

const stage = (
  range: { min: number; max: number; step?: number },
  values?: number[],
  kind: GainStage["kind"] = "lna",
): GainStage => ({ name: "TEST", kind, range, ...(values == null ? {} : { values }) });

const TUNER = stage(
  { min: 0, max: 49.6 },
  [
    0, 0.9, 1.4, 2.7, 3.7, 7.7, 8.7, 12.5, 14.4, 15.7, 16.6, 19.7, 20.7, 22.9, 25.4, 28, 29.7, 32.8,
    33.8, 36.4, 37.2, 38.6, 40.2, 42.1, 43.4, 43.9, 44.5, 48, 49.6,
  ],
);

describe("isSwitch", () => {
  it("is an amp and nothing else", () => {
    expect(isSwitch(stage({ min: 0, max: 14, step: 14 }, undefined, "amp"))).toBe(true);
    expect(isSwitch(stage({ min: 0, max: 14, step: 14 }))).toBe(false);
    expect(isSwitch(stage({ min: 0, max: 14 }, [0, 14]))).toBe(false);
    expect(isSwitch(TUNER)).toBe(false);
  });
});

describe("gain labels", () => {
  it("names a stage by its kind and falls back to the hardware name", () => {
    expect(gainLabel({ kind: "lna", name: "LNA" })).toBe("LNA");
    expect(gainLabel({ kind: "mixer", name: "MIX" })).toBe("Mixer");
    expect(gainLabel({ kind: "other", name: "rxvga1" })).toBe("rxvga1");
  });

  it("reads a firmware step without a unit", () => {
    expect(gainUnit({ unit: "index" })).toBe("");
    expect(gainUnit({})).toBe("dB");
    expect(formatGain({ unit: "index" }, 7.4)).toBe("7");
    expect(formatGain({ unit: "db" }, 7.4)).toBe("7.4");
  });
});

describe("stageSettings", () => {
  it("walks an even step without overshooting the top", () => {
    expect(stageSettings(stage({ min: 0, max: 40, step: 8 }))).toEqual([0, 8, 16, 24, 32, 40]);
  });

  it("returns a declared table in order", () => {
    expect(stageSettings(stage({ min: 0, max: 14 }, [14, 0]))).toEqual([0, 14]);
  });

  it("has nothing to offer for a continuous stage", () => {
    expect(stageSettings(stage({ min: 0, max: 10 }))).toEqual([]);
  });
});

describe("snapToStage", () => {
  it("lands only on settings the radio can hold", () => {
    expect(snapToStage(TUNER, 20)).toBe(19.7);
    expect(snapToStage(TUNER, 19.7)).toBe(19.7);
    expect(snapToStage(TUNER, 21)).toBe(20.7);
  });

  it("clamps rather than inventing a setting past the ends", () => {
    expect(snapToStage(TUNER, -5)).toBe(0);
    expect(snapToStage(TUNER, 1000)).toBe(49.6);
  });

  it("breaks a tie downward so a snap never raises gain", () => {
    expect(snapToStage(stage({ min: 0, max: 20 }, [0, 10, 20]), 15)).toBe(10);
  });

  it("only clamps a stage with no grid at all", () => {
    expect(snapToStage(stage({ min: 0, max: 10 }), 3.7)).toBe(3.7);
    expect(snapToStage(stage({ min: 0, max: 10 }), 11)).toBe(10);
  });
});

describe("settingIndex", () => {
  it("finds where a value sits so a slider can address it", () => {
    const settings = stageSettings(TUNER);
    expect(settings[settingIndex(settings, 19.7)]).toBe(19.7);
    expect(settings[settingIndex(settings, 20)]).toBe(19.7);
    expect(settings[settingIndex(settings, -99)]).toBe(0);
    expect(settings[settingIndex(settings, 99)]).toBe(49.6);
  });
});

describe("spanOf", () => {
  it("covers every window a radio declares", () => {
    expect(
      spanOf([
        { min: 225001, max: 300000 },
        { min: 900001, max: 3200000 },
      ]),
    ).toEqual({ min: 225001, max: 3200000, step: undefined });
  });

  it("keeps a step only when every window agrees there is one", () => {
    expect(spanOf([{ min: 2e6, max: 20e6, step: 1000 }])).toEqual({
      min: 2e6,
      max: 20e6,
      step: 1000,
    });
    expect(
      spanOf([
        { min: 0, max: 1, step: 1 },
        { min: 2, max: 3 },
      ])?.step,
    ).toBeUndefined();
  });

  it("has no span without windows", () => {
    expect(spanOf([])).toBeUndefined();
    expect(spanOf(undefined)).toBeUndefined();
  });
});

describe("snapToRanges", () => {
  const WINDOWS = [
    { min: 225001, max: 300000 },
    { min: 900001, max: 3200000 },
  ];

  it("leaves a value that a window already holds", () => {
    expect(snapToRanges(WINDOWS, 250000)).toBe(250000);
    expect(snapToRanges(WINDOWS, 2048000)).toBe(2048000);
  });

  it("moves a value in the gap to the nearest edge rather than offering it", () => {
    expect(snapToRanges(WINDOWS, 400000)).toBe(300000);
    expect(snapToRanges(WINDOWS, 800000)).toBe(900001);
  });

  it("clamps past either end", () => {
    expect(snapToRanges(WINDOWS, 1000)).toBe(225001);
    expect(snapToRanges(WINDOWS, 9e9)).toBe(3200000);
  });

  it("has nothing to say without windows", () => {
    expect(snapToRanges([], 500)).toBe(500);
    expect(snapToRanges(undefined, 500)).toBe(500);
  });
});

describe("hasDcArtifact", () => {
  it("offers the blocker to every radio with a front end", () => {
    expect(hasDcArtifact({ dc_artifact: "operator" })).toBe(true);
    expect(hasDcArtifact({ dc_artifact: "managed" })).toBe(true);
    expect(hasDcArtifact({})).toBe(true);
  });

  it("keeps it from a source with no front end at all", () => {
    expect(hasDcArtifact({ dc_artifact: "none" })).toBe(false);
  });
});

describe("dcBlockOn", () => {
  it("starts on for hardware known to land a DC term, off otherwise", () => {
    expect(dcBlockOn({ dc_artifact: "managed" }, {})).toBe(true);
    expect(dcBlockOn({ dc_artifact: "operator" }, {})).toBe(false);
  });

  it("follows the operator's choice either way", () => {
    expect(dcBlockOn({ dc_artifact: "managed" }, { dc_block: false })).toBe(false);
    expect(dcBlockOn({ dc_artifact: "operator" }, { dc_block: true })).toBe(true);
  });
});

describe("automaticGainIsOn", () => {
  const modes = {
    agc: { kind: "modes" as const, options: [{ value: "fast" }, { value: "slow" }] },
  };

  it("reads the state the radio reports", () => {
    expect(automaticGainIsOn({ agc: { kind: "switch" } }, { agc: { on: true } })).toBe(true);
    expect(automaticGainIsOn({ agc: { kind: "switch" } }, { agc: { on: false } })).toBe(false);
  });

  it("leaves a radio with no such control alone", () => {
    expect(automaticGainIsOn({ agc: { kind: "none" } }, { agc: { on: true } })).toBe(false);
    expect(automaticGainIsOn({}, { agc: { on: true } })).toBe(false);
    expect(automaticGainIsOn(modes, {})).toBe(false);
  });

  it("offers the first mode until the radio names one", () => {
    expect(agcState(modes, {})).toEqual({ on: false, mode: "fast" });
    expect(agcState(modes, { agc: { on: true, mode: "slow" } })).toEqual({
      on: true,
      mode: "slow",
    });
    expect(agcState({ agc: { kind: "switch" } }, { agc: { on: true } })).toEqual({ on: true });
  });
});

describe("filter", () => {
  const menu = { bandwidths: [1.75e6, 2.5e6, 5e6], bandwidth_ranges: [] };

  it("is offered for a menu, a range or an automatic width", () => {
    expect(hasFilter({ bandwidths: [], bandwidth_ranges: [], bandwidth_auto: true })).toBe(true);
    expect(hasFilter({ ...menu })).toBe(true);
    expect(hasFilter({ bandwidths: [], bandwidth_ranges: [{ min: 1e6, max: 8e6 }] })).toBe(true);
    expect(hasFilter({ bandwidths: [], bandwidth_ranges: [] })).toBe(false);
  });

  it("shows the manual width, or the narrowest one that covers the rate under auto", () => {
    expect(filterHz(menu, { bandwidth: { kind: "manual", hz: 5e6 } })).toBe(5e6);
    expect(filterHz(menu, { bandwidth: { kind: "auto" }, sample_rate: 2.4e6 })).toBe(2.5e6);
    expect(filterHz(menu, { sample_rate: 20e6 })).toBe(5e6);
    expect(
      filterHz(
        { bandwidths: [], bandwidth_ranges: [{ min: 290e3, max: 8e6 }] },
        { sample_rate: 2.4e6 },
      ),
    ).toBe(2.4e6);
  });
});

describe("fitsSlider", () => {
  it("takes a short stepped range, like an LNA state", () => {
    expect(fitsSlider({ min: 0, max: 9, step: 1 })).toBe(true);
    expect(fitsSlider({ min: 0, max: 100, step: 1 })).toBe(true);
  });

  it("leaves a wide or continuous range to a number field", () => {
    expect(fitsSlider({ min: 1e3, max: 2e9, step: 1 })).toBe(false);
    expect(fitsSlider({ min: 0, max: 200e3, step: 1 })).toBe(false);
    expect(fitsSlider({ min: 0, max: 10 })).toBe(false);
    expect(fitsSlider({ min: 0, max: 10, step: 0 })).toBe(false);
    expect(fitsSlider({ min: 5, max: 5, step: 1 })).toBe(false);
  });
});
