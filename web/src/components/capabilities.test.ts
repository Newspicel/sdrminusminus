import { describe, expect, it } from "vitest";
import type { GainStage } from "../lib/types";
import {
  clampLoOffsetHz,
  isSwitch,
  loOffsetLimitHz,
  managesDcArtifact,
  settingIndex,
  snapToRanges,
  snapToStage,
  spanOf,
  stageSettings,
} from "./capabilities";

const stage = (
  range: { min: number; max: number; step?: number },
  values?: number[],
): GainStage => ({ name: "TEST", range, ...(values == null ? {} : { values }) });

const TUNER = stage(
  { min: 0, max: 49.6 },
  [
    0, 0.9, 1.4, 2.7, 3.7, 7.7, 8.7, 12.5, 14.4, 15.7, 16.6, 19.7, 20.7, 22.9, 25.4, 28, 29.7, 32.8,
    33.8, 36.4, 37.2, 38.6, 40.2, 42.1, 43.4, 43.9, 44.5, 48, 49.6,
  ],
);

describe("isSwitch", () => {
  it("reads a two-setting stage as a switch however it was declared", () => {
    expect(isSwitch(stage({ min: 0, max: 14, step: 14 }))).toBe(true);
    expect(isSwitch(stage({ min: 0, max: 14 }, [0, 14]))).toBe(true);
  });

  it("leaves controls with room to move alone", () => {
    expect(isSwitch(stage({ min: 0, max: 40, step: 8 }))).toBe(false);
    expect(isSwitch(stage({ min: 0, max: 62, step: 2 }))).toBe(false);
    expect(isSwitch(stage({ min: 0, max: 10 }))).toBe(false);
    expect(isSwitch(TUNER)).toBe(false);
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

describe("loOffsetLimitHz", () => {
  it("keeps the LO inside the tuner's flat passband", () => {
    expect(loOffsetLimitHz(2_400_000)).toBe(960_000);
    expect(loOffsetLimitHz(0)).toBe(0);
    expect(loOffsetLimitHz(undefined)).toBe(0);
    expect(loOffsetLimitHz(Number.NaN)).toBe(0);
  });
});

describe("clampLoOffsetHz", () => {
  it("holds a request to the limit in both directions", () => {
    expect(clampLoOffsetHz(250_000, 2_400_000)).toBe(250_000);
    expect(clampLoOffsetHz(5_000_000, 2_400_000)).toBe(960_000);
    expect(clampLoOffsetHz(-5_000_000, 2_400_000)).toBe(-960_000);
  });

  it("falls back to tuning dead centre when there is no room or no number", () => {
    expect(clampLoOffsetHz(250_000, undefined)).toBe(0);
    expect(clampLoOffsetHz(Number.NaN, 2_400_000)).toBe(0);
  });
});

describe("managesDcArtifact", () => {
  it("keeps the controls for hardware the engine does not recognise", () => {
    expect(managesDcArtifact({ dc_artifact: "operator" })).toBe(false);
    expect(managesDcArtifact({})).toBe(false);
  });

  it("drops them for a front end the engine handles itself", () => {
    expect(managesDcArtifact({ dc_artifact: "managed" })).toBe(true);
  });
});
