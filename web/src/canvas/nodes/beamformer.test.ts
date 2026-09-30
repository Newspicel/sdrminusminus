import { describe, expect, it } from "vitest";
import type { BeamformerParams, BeamformerReading } from "../../lib/types";
import { CATALOG } from "../../test/catalog";
import { defaultSettings } from "../newNode";
import {
  amplitudePercent,
  beamChips,
  outText,
  patternPath,
  referenceLanes,
  steerLabel,
  toggledReference,
  visibleSettings,
  weightTitle,
  withNull,
  withoutNull,
} from "./beamformer";

function defaults(): BeamformerParams {
  const settings = defaultSettings(CATALOG, "beamformer");
  if (settings === null) {
    throw new Error("the catalog has no beamformer");
  }
  return settings;
}

function reading(overrides: Partial<BeamformerReading> = {}): BeamformerReading {
  return { at: "2026-09-28T12:00:00Z", output_db: -42, nulls_deg: [], weights: [], ...overrides };
}

describe("beamformer weights", () => {
  it("normalises weight bars to the strongest lane", () => {
    expect(
      amplitudePercent([
        { amplitude_db: 0, phase_deg: 0 },
        { amplitude_db: -6.0206, phase_deg: 10 },
        { amplitude_db: -20, phase_deg: 20 },
      ]),
    ).toEqual([100, 50, 10]);
    expect(amplitudePercent([])).toEqual([]);
  });

  it("titles a weight with lane, level and phase", () => {
    expect(weightTitle(0, { amplitude_db: -1.24, phase_deg: 36.6 })).toBe("L1 -1.2 dB 37°");
  });
});

describe("beamformer nulls", () => {
  it("keeps nulls unique, wrapped and sorted", () => {
    expect(withNull([90, 45], 405)).toEqual([45, 90]);
    expect(withNull([90], -10)).toEqual([90, 350]);
    expect(withNull([10.04], 10.01)).toEqual([10]);
    expect(withNull([10], Number.NaN)).toEqual([10]);
    expect(withoutNull([45, 90], 45)).toEqual([90]);
  });
});

describe("beamformer settings", () => {
  it("shows only the settings the mode uses", () => {
    const base = defaults();
    expect([...visibleSettings(base)].toSorted()).toEqual(
      ["carry_over", "crossfade", "noise", "update"].toSorted(),
    );
    const lcmv = visibleSettings({ ...base, mode: "lcmv" });
    expect(lcmv.has("nulls")).toBe(true);
    expect(lcmv.has("steer")).toBe(true);
    expect(lcmv.has("timeout")).toBe(true);
    const fixed = visibleSettings({
      ...base,
      mode: "mvdr",
      steer: { kind: "fixed", azimuth_deg: 10, elevation_deg: 0 },
    });
    expect(fixed.has("timeout")).toBe(false);
    const rls = visibleSettings({ ...base, mode: "canceller", taps: 4, adaptation: "rls" });
    expect(rls.has("forget")).toBe(true);
    expect(rls.has("step")).toBe(false);
    expect(visibleSettings({ ...base, mode: "canceller", taps: 1 }).has("adaptation")).toBe(false);
    expect([...visibleSettings({ ...base, mode: "cma" })]).toEqual(["step"]);
  });

  it("toggles reference lanes and folds back to all others", () => {
    const base = { ...defaults(), mode: "canceller" as const, main_lane: 0 };
    expect(referenceLanes(base, 4)).toEqual([1, 2, 3]);
    const fewer = toggledReference(base, 2, 4);
    expect(fewer).toEqual([1, 3]);
    expect(toggledReference({ ...base, reference_lanes: fewer }, 2, 4)).toEqual([]);
    expect(toggledReference({ ...base, reference_lanes: [1] }, 1, 4)).toEqual([1]);
  });
});

describe("beamformer readout", () => {
  it("says where the steer comes from", () => {
    expect(steerLabel(reading({ steer_deg: 137.4 }), { kind: "wired" })).toBe("137° DF");
    expect(
      steerLabel(reading({ steer_deg: 90 }), { kind: "fixed", azimuth_deg: 90, elevation_deg: 0 }),
    ).toBe("90° fixed");
    expect(steerLabel(reading(), { kind: "wired" })).toBe("-");
  });

  it("names the beam lane's centre and rate", () => {
    expect(outText(reading({ out_center_hz: 433_920_000, out_rate: 250_000 }))).toBe(
      "433.920 MHz 250 kS/s",
    );
    expect(outText(reading())).toBe("-");
    expect(outText(null)).toBe("-");
  });

  it("raises a chip for every flag", () => {
    const labels = beamChips(
      reading({
        no_steer: true,
        steer_stale: true,
        singular: true,
        resets: 2,
        band_full: true,
        steer_deg: 40,
        nulls_deg: [45],
      }),
    ).map((chip) => chip.label);
    expect(labels).toEqual(["No steer", "Stale", "Singular", "Reset", "Band full", "Null close"]);
    expect(
      beamChips(reading({ steer_deg: 355, nulls_deg: [3] })).map((chip) => chip.label),
    ).toEqual(["Null close"]);
    expect(beamChips(null)).toEqual([]);
  });

  it("closes the pattern path", () => {
    const path = patternPath([255, 128, 0, 128], 50, 60);
    expect(path.startsWith("M60.0 10.0")).toBe(true);
    expect(path.endsWith("Z")).toBe(true);
    expect(patternPath([], 50, 60)).toBe("");
  });
});
