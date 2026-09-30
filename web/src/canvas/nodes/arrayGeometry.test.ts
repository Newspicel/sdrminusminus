import { describe, expect, it } from "vitest";
import type { ArrayGeometry } from "../../lib/types";
import {
  adjacentSpacingM,
  allowsStructured,
  convertGeometry,
  elementPositions,
  isCollinear,
  isLevelLine,
  movedElement,
  overlappingRows,
  paddedPositions,
  previewPoints,
  unambiguousHz,
} from "./arrayGeometry";

const CIRCLE: ArrayGeometry = { kind: "uca", radius_m: 0.35, first_deg: 0, winding: "clockwise" };

function near(actual: { x: number; y: number; z: number } | undefined, x: number, y: number) {
  expect(actual?.x).toBeCloseTo(x, 9);
  expect(actual?.y).toBeCloseTo(y, 9);
  expect(actual?.z).toBe(0);
}

describe("elementPositions", () => {
  it("puts lane 1 forward and runs clockwise on a circle", () => {
    const unit: ArrayGeometry = { kind: "uca", radius_m: 1 };
    const positions = elementPositions(unit, 4);
    near(positions[0], 0, 1);
    near(positions[1], 1, 0);
    near(positions[2], 0, -1);
    near(positions[3], -1, 0);
    const turned = elementPositions({ ...unit, first_deg: 90, winding: "counter_clockwise" }, 4);
    near(turned[0], 1, 0);
    near(turned[1], 0, 1);
  });

  it("centres a line on its axis", () => {
    const line: ArrayGeometry = { kind: "ula", spacing_m: 0.5, axis_deg: 90 };
    const positions = elementPositions(line, 3);
    near(positions[0], -0.5, 0);
    near(positions[1], 0, 0);
    near(positions[2], 0.5, 0);
    near(elementPositions({ ...line, axis_deg: 0 }, 2)[1], 0, 0.25);
  });

  it("draws two elements for an array with fewer lanes and custom rows as wired", () => {
    expect(elementPositions(CIRCLE, 0)).toHaveLength(2);
    const custom: ArrayGeometry = {
      kind: "explicit",
      positions: [
        { x_m: 0, y_m: 0 },
        { x_m: 1, y_m: 0, z_m: 2 },
        { x_m: 3, y_m: 0 },
      ],
    };
    expect(elementPositions(custom, 2)).toEqual([
      { x: 0, y: 0, z: 0 },
      { x: 1, y: 0, z: 2 },
    ]);
  });
});

describe("convertGeometry", () => {
  it("keeps element spacing when switching shape", () => {
    const line = convertGeometry("ula", CIRCLE, 5);
    expect(line.kind).toBe("ula");
    expect(line.kind === "ula" ? line.spacing_m : 0).toBeCloseTo(0.4114, 4);
    expect(line.kind === "ula" ? line.axis_deg : 0).toBe(90);
    const back = convertGeometry("uca", line, 5);
    expect(back.kind === "uca" ? back.radius_m : 0).toBeCloseTo(0.35, 9);
    expect(convertGeometry("uca", CIRCLE, 5)).toBe(CIRCLE);
  });

  it("writes the current layout as custom rows rounded to a millimetre", () => {
    const custom = convertGeometry("explicit", { kind: "uca", radius_m: 1 }, 4);
    expect(custom).toEqual({
      kind: "explicit",
      positions: [
        { x_m: 0, y_m: 1, z_m: 0 },
        { x_m: 1, y_m: 0, z_m: 0 },
        { x_m: 0, y_m: -1, z_m: 0 },
        { x_m: -1, y_m: 0, z_m: 0 },
      ],
    });
  });

  it("falls back to half a metre when custom rows share one place", () => {
    const stacked: ArrayGeometry = {
      kind: "explicit",
      positions: [
        { x_m: 0, y_m: 0 },
        { x_m: 0, y_m: 0 },
      ],
    };
    expect(convertGeometry("ula", stacked, 2)).toEqual({
      kind: "ula",
      spacing_m: 0.5,
      axis_deg: 90,
    });
  });
});

describe("unambiguousHz", () => {
  it("reports the unambiguous frequency", () => {
    expect(adjacentSpacingM(CIRCLE, 5)).toBeCloseTo(0.41145, 4);
    expect((unambiguousHz(CIRCLE, 5) ?? 0) / 1e6).toBeCloseTo(364.3, 1);
    expect(unambiguousHz({ kind: "ula", spacing_m: 0.5, axis_deg: 90 }, 4)).toBeCloseTo(
      299_792_458,
      0,
    );
    expect(unambiguousHz({ kind: "explicit", positions: [{ x_m: 0, y_m: 0 }] }, 1)).toBeNull();
  });
});

describe("custom rows", () => {
  it("pads rows up to the wired lanes and edits one axis of one row", () => {
    const padded = paddedPositions([{ x_m: 0.1, y_m: 0.2 }], 3);
    expect(padded).toEqual([
      { x_m: 0.1, y_m: 0.2 },
      { x_m: 0, y_m: 0, z_m: 0 },
      { x_m: 0, y_m: 0, z_m: 0 },
    ]);
    expect(paddedPositions(padded, 1)).toHaveLength(3);
    expect(movedElement(padded, 0, "z_m", 1.5)[0]).toEqual({ x_m: 0.1, y_m: 0.2, z_m: 1.5 });
    const moved = movedElement(padded, 2, "x_m", -0.3);
    expect(moved[2]).toEqual({ x_m: -0.3, y_m: 0, z_m: 0 });
    expect(moved[0]).toBe(padded[0]);
  });
});

describe("overlappingRows", () => {
  it("flags overlapping custom rows", () => {
    const positions = [
      { x_m: 0, y_m: 0 },
      { x_m: 0.3, y_m: 0 },
      { x_m: 0.002, y_m: 0.002, z_m: 0 },
      { x_m: 0.3, y_m: 0 },
    ];
    expect(overlappingRows(positions, 4)).toEqual([2, 3]);
    expect(overlappingRows(positions, 3)).toEqual([2]);
    expect(overlappingRows(positions, 4, 0.001)).toEqual([3]);
  });
});

function bentRow(off: number): ArrayGeometry {
  return {
    kind: "explicit",
    positions: [
      { x_m: 0, y_m: 0 },
      { x_m: 0.5, y_m: off },
      { x_m: 1, y_m: 0 },
    ],
  };
}

describe("line shapes", () => {
  it("knows a line from a plane", () => {
    expect(isCollinear({ kind: "ula", spacing_m: 0.5, axis_deg: 0 }, 4)).toBe(true);
    expect(isCollinear(CIRCLE, 5)).toBe(false);
    expect(isCollinear(CIRCLE, 2)).toBe(true);
    const diagonal: ArrayGeometry = {
      kind: "explicit",
      positions: [
        { x_m: 0, y_m: 0 },
        { x_m: 1, y_m: 1 },
        { x_m: 2, y_m: 2 },
      ],
    };
    expect(isCollinear(diagonal, 3)).toBe(true);
  });

  it("allows the same slack as the server, a thousandth of the aperture", () => {
    expect(isCollinear(bentRow(0.0009), 3)).toBe(true);
    expect(isCollinear(bentRow(0.0011), 3)).toBe(false);
  });

  it("needs a level line for a side and an even one for grid-free methods", () => {
    const mast: ArrayGeometry = {
      kind: "explicit",
      positions: [
        { x_m: 0, y_m: 0, z_m: 0 },
        { x_m: 0, y_m: 0, z_m: 1 },
        { x_m: 0, y_m: 0, z_m: 2 },
      ],
    };
    expect(isCollinear(mast, 3)).toBe(true);
    expect(isLevelLine(mast, 3)).toBe(false);
    expect(allowsStructured(mast, 3)).toBe(false);
    const shuffled: ArrayGeometry = {
      kind: "explicit",
      positions: [
        { x_m: 0.6, y_m: 0 },
        { x_m: -0.6, y_m: 0 },
        { x_m: 0, y_m: 0 },
        { x_m: 0.2, y_m: 0 },
      ],
    };
    expect(isLevelLine(shuffled, 4)).toBe(true);
    expect(allowsStructured(shuffled, 4)).toBe(false);
    const spaced = {
      kind: "explicit" as const,
      positions: [
        { x_m: 0.6, y_m: 0 },
        { x_m: -0.6, y_m: 0 },
        { x_m: 0.2, y_m: 0 },
        { x_m: -0.2, y_m: 0 },
      ],
    };
    expect(allowsStructured(spaced, 4)).toBe(true);
    expect(allowsStructured(CIRCLE, 5)).toBe(true);
    expect(isLevelLine(CIRCLE, 5)).toBe(false);
  });
});

describe("previewPoints", () => {
  it("fills the box and flips y so forward is up", () => {
    const points = previewPoints({ kind: "uca", radius_m: 2 }, 4, 100);
    expect(points[0]).toEqual({ x: 50, y: 14, lane: 1 });
    expect(points[1]?.x).toBeCloseTo(86, 9);
    expect(points[1]?.lane).toBe(2);
    const stacked = previewPoints({ kind: "explicit", positions: [{ x_m: 0, y_m: 0 }] }, 1, 96);
    expect(stacked).toEqual([{ x: 48, y: 48, lane: 1 }]);
  });
});
