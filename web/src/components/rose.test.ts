import { describe, expect, it } from "vitest";
import { distinctNeedles, polarPoint, sigmaWedge, spectrumPath } from "./Rose";

function start(path: string): [number, number] {
  const match = /^M(-?[\d.]+) (-?[\d.]+)/.exec(path);
  return [Number(match?.[1]), Number(match?.[2])];
}

describe("polarPoint", () => {
  it("puts north at the top and runs clockwise", () => {
    const north = polarPoint(0, 10, 50);
    expect(north.x).toBeCloseTo(50, 9);
    expect(north.y).toBeCloseTo(40, 9);
    const east = polarPoint(90, 10, 50);
    expect(east.x).toBeCloseTo(60, 9);
    expect(east.y).toBeCloseTo(50, 9);
  });
});

describe("spectrumPath", () => {
  it("closes the spectrum path", () => {
    const path = spectrumPath([255, 0, 0, 0], 50, 10, 40);
    expect(path.startsWith("M50.00 10.00")).toBe(true);
    expect(path.endsWith(" Z")).toBe(true);
    expect(path.match(/L/g)).toHaveLength(3);
    expect(spectrumPath([], 50, 10, 40)).toBe("");
  });

  it("turns the whole trace by the rotation", () => {
    const [x, y] = start(spectrumPath([255, 0, 0, 0], 50, 10, 40, 90));
    expect(x).toBeCloseTo(90, 2);
    expect(y).toBeCloseTo(50, 2);
  });
});

describe("sigmaWedge", () => {
  it("opens symmetrically around the bearing", () => {
    const path = sigmaWedge(0, 90, 50, 40);
    expect(path).toBe("M50.00 50.00 L10.00 50.00 A40 40 0 0 1 90.00 50.00 Z");
    expect(sigmaWedge(0, 120, 50, 40)).toContain("A40 40 0 1 1");
  });
});

describe("distinctNeedles", () => {
  it("draws one needle per weight and bearing", () => {
    const needles = distinctNeedles([
      { deg: 10, weight: "primary" },
      { deg: 10, weight: "mirror" },
      { deg: 10, weight: "primary" },
    ]);
    expect(needles.map(([key]) => key)).toEqual(["primary:10.00", "mirror:10.00"]);
  });
});
