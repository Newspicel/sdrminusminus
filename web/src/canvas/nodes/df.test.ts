import { describe, expect, it } from "vitest";
import type { ArrayGeometry, CalState } from "../../lib/types";
import {
  beamAzimuth,
  beamMode,
  bearingLabel,
  CAL_VERDICT_TEXT,
  calVerdict,
  DEFAULT_DF_PARAMS,
  elementCount,
  geometryOf,
  laneQualityPercent,
  polarPoint,
  spectrumPath,
  tierLabel,
  withCount,
} from "./df";

function cal(over: Partial<CalState> = {}): CalState {
  return { tier: "phase_coherent", lanes: [], phase_unknown: false, solved: true, ...over };
}

describe("elementCount", () => {
  it("counts what each geometry actually places", () => {
    expect(elementCount({ kind: "uca", radius_m: 0.35, count: 4 })).toBe(4);
    expect(elementCount({ kind: "ula", spacing_m: 0.5, count: 6 })).toBe(6);
    expect(
      elementCount({
        kind: "explicit",
        positions: [
          { x_m: 0, y_m: 1 },
          { x_m: 1, y_m: 0 },
        ],
      }),
    ).toBe(2);
  });
});

describe("geometryOf", () => {
  it("keeps the element count when the shape changes", () => {
    const line = geometryOf("ula", { kind: "uca", radius_m: 0.35, count: 6 });
    expect(line).toEqual({ kind: "ula", spacing_m: 0.5, count: 6 });
    expect(elementCount(geometryOf("uca", line))).toBe(6);
  });

  it("leaves explicit positions alone when the count is edited", () => {
    const explicit: ArrayGeometry = { kind: "explicit", positions: [{ x_m: 0, y_m: 0 }] };
    expect(withCount(explicit, 8)).toEqual(explicit);
    expect(withCount({ kind: "uca", radius_m: 1, count: 2 }, 8)).toEqual({
      kind: "uca",
      radius_m: 1,
      count: 8,
    });
  });
});

describe("polarPoint", () => {
  it("puts north at the top and runs clockwise", () => {
    const centre = 100;
    const north = polarPoint(0, 50, centre);
    expect(north.x).toBeCloseTo(100, 6);
    expect(north.y).toBeCloseTo(50, 6);
    const east = polarPoint(90, 50, centre);
    expect(east.x).toBeCloseTo(150, 6);
    expect(east.y).toBeCloseTo(100, 6);
    const south = polarPoint(180, 50, centre);
    expect(south.y).toBeCloseTo(150, 6);
  });
});

describe("spectrumPath", () => {
  it("closes a ring with one point per sample", () => {
    const path = spectrumPath([0, 128, 255, 128], 100, 20, 80);
    expect(path.startsWith("M")).toBe(true);
    expect(path.endsWith("Z")).toBe(true);
    expect(path.split("L")).toHaveLength(4);
  });

  it("has nothing to draw for an empty surface", () => {
    expect(spectrumPath([], 100, 20, 80)).toBe("");
  });
});

describe("calVerdict", () => {
  it("refuses to call an unknown phase anything else", () => {
    expect(calVerdict(undefined)).toBe("phase_unknown");
    expect(calVerdict(cal({ phase_unknown: true }))).toBe("phase_unknown");
    expect(calVerdict(cal({ solved: false }))).toBe("solving");
    expect(calVerdict(cal())).toBe("solved");
    expect(CAL_VERDICT_TEXT.phase_unknown).toContain("no bearings");
  });
});

describe("tierLabel", () => {
  it("says what the hardware actually shares", () => {
    expect(tierLabel(cal({ tier: "phase_coherent" }))).toBe("shared LO");
    expect(tierLabel(cal({ tier: "time_sync" }))).toBe("shared clock");
    expect(tierLabel(cal({ tier: "none" }))).toBe("not coherent");
    expect(tierLabel(undefined)).toBe("not coherent");
  });
});

describe("bearingLabel", () => {
  it("pads a bearing so the readout never jumps width", () => {
    expect(bearingLabel(7.24)).toBe("007.2°");
    expect(bearingLabel(137.5)).toBe("137.5°");
  });
});

describe("laneQualityPercent", () => {
  it("clamps to a bar width", () => {
    expect(laneQualityPercent(-1)).toBe(0);
    expect(laneQualityPercent(0.5)).toBe(50);
    expect(laneQualityPercent(3)).toBe(100);
  });
});

describe("DEFAULT_DF_PARAMS", () => {
  it("describes an array the server will accept", () => {
    expect(elementCount(DEFAULT_DF_PARAMS.geometry)).toBeGreaterThanOrEqual(2);
    expect(DEFAULT_DF_PARAMS.sources).toBeLessThan(elementCount(DEFAULT_DF_PARAMS.geometry));
    expect(DEFAULT_DF_PARAMS.report_ms).toBeGreaterThanOrEqual(100);
  });
});

describe("beam steering", () => {
  it("follows the bearing until the operator pins it", () => {
    expect(beamMode(null)).toBe("follow");
    expect(beamMode(0)).toBe("fixed");
    expect(beamAzimuth("follow", 137)).toBeNull();
  });

  it("pins the beam where the array is already pointing", () => {
    expect(beamAzimuth("fixed", 137.4)).toBe(137);
    expect(beamAzimuth("fixed", 359.7)).toBe(0);
    expect(beamAzimuth("fixed", null)).toBe(0);
  });
});
