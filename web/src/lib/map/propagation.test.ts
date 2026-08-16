import { describe, expect, it } from "vitest";
import type { PropagationCell, PropagationPath } from "../propagation";
import type { IonosondeStation } from "../types";
import {
  cellCollection,
  greatCircleLine,
  MUF_MAX_MHZ,
  MUF_MIN_MHZ,
  mufColor,
  pathCollection,
  sondeCollection,
} from "./propagation";

function cell(over: Partial<PropagationCell> = {}): PropagationCell {
  return {
    key: "IO91",
    latitude: 51.5,
    longitude: -1,
    weight: 2.5,
    decodes: 4,
    callsigns: 3,
    bestFreqHz: 14_074_000,
    bestSnrDb: -8,
    measuredMuf3000Mhz: 18,
    medianDistanceKm: 3_000,
    lastSeen: 0,
    ...over,
  };
}

function path(over: Partial<PropagationPath> = {}): PropagationPath {
  return {
    key: "p",
    from: [52.5, 13],
    to: [42.5, -71],
    weight: 0.8,
    freqHz: 14_074_000,
    ...over,
  };
}

describe("propagation map sources", () => {
  it("puts a cell at its own square with the numbers a layer paints from", () => {
    const collection = cellCollection([cell()]);
    expect(collection.features).toHaveLength(1);
    expect(collection.features[0]?.geometry.coordinates).toEqual([-1, 51.5]);
    expect(collection.features[0]?.properties).toEqual({
      grid: "IO91",
      weight: 2.5,
      decodes: 4,
      muf: 18,
      hasMuf: true,
    });
  });

  it("flags a cell that has no measured MUF so the MUF layer can skip it", () => {
    const collection = cellCollection([cell({ measuredMuf3000Mhz: null })]);
    expect(collection.features[0]?.properties.hasMuf).toBe(false);
    expect(collection.features[0]?.properties.muf).toBe(0);
  });

  it("draws a path as a curve that starts and ends where it should", () => {
    const collection = pathCollection([path()]);
    const line = collection.features[0]?.geometry.coordinates ?? [];
    expect(line.length).toBeGreaterThan(2);
    expect(line[0]?.[0]).toBeCloseTo(13, 6);
    expect(line[0]?.[1]).toBeCloseTo(52.5, 6);
    expect(line.at(-1)?.[1]).toBeCloseTo(42.5, 6);
    expect(collection.features[0]?.properties.weight).toBe(0.8);
  });

  it("unwraps a path that crosses the antimeridian into one continuous line", () => {
    const line =
      pathCollection([path({ from: [0, 170], to: [0, -170] })]).features[0]?.geometry.coordinates ??
      [];
    const jumps = line
      .slice(1)
      .filter((point, index) => Math.abs(point[0] - (line[index]?.[0] ?? 0)) > 180);
    expect(jumps).toHaveLength(0);
  });

  it("labels an ionosonde with its MUF", () => {
    const sonde: IonosondeStation = {
      code: "AU930",
      name: "Austin",
      latitude: 30.4,
      longitude: -97.7,
      muf3000_mhz: 28.8,
      measured_at: "2026-08-16T18:10:05Z",
    };
    const collection = sondeCollection([sonde]);
    expect(collection.features[0]?.geometry.coordinates).toEqual([-97.7, 30.4]);
    expect(collection.features[0]?.properties.label).toBe("28.8");
  });
});

describe("great-circle line", () => {
  it("stays on the sphere between its ends", () => {
    const line = greatCircleLine([0, 0], [0, 80]);
    expect(line[0]).toEqual([0, 0]);
    expect(line.at(-1)?.[0]).toBeCloseTo(80, 6);
    expect(line[Math.floor(line.length / 2)]?.[0]).toBeCloseTo(40, 6);
  });

  it("collapses a zero-length path rather than dividing by nothing", () => {
    const line = greatCircleLine([10, 20], [10, 20]);
    expect(line.every(([lon, lat]) => lon === 20 && lat === 10)).toBe(true);
  });
});

describe("the MUF colour ramp", () => {
  it("pins its ends and moves through the middle", () => {
    expect(mufColor(MUF_MIN_MHZ)).toBe("#3b4a7a");
    expect(mufColor(MUF_MIN_MHZ - 10)).toBe("#3b4a7a");
    expect(mufColor(MUF_MAX_MHZ)).toBe("#ef6262");
    expect(mufColor(MUF_MAX_MHZ + 10)).toBe("#ef6262");
    expect(mufColor(18)).toBe("#3fae7a");
    expect(mufColor(14)).not.toBe(mufColor(18));
  });
});
