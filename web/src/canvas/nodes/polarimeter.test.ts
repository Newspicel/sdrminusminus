import { describe, expect, it } from "vitest";
import type { PolarimeterParams } from "../../lib/types";
import { CATALOG } from "../../test/catalog";
import { defaultSettings } from "../newNode";
import {
  ellipsePath,
  ellipsePoint,
  handText,
  laneEdit,
  percentText,
  senseArrow,
  senseStep,
} from "./polarimeter";

function points(path: string): [number, number][] {
  return [...path.matchAll(/[ML](-?[\d.]+) (-?[\d.]+)/g)].map((match) => [
    Number(match[1]),
    Number(match[2]),
  ]);
}

function defaults(): PolarimeterParams {
  const settings = defaultSettings(CATALOG, "polarimeter");
  if (settings === null) {
    throw new Error("the catalog has no polarimeter");
  }
  return settings;
}

describe("ellipsePath", () => {
  it("closes the path with one point per step", () => {
    const path = ellipsePath(30, 10, 40, 48);
    expect(path.endsWith("Z")).toBe(true);
    expect(points(path)).toHaveLength(64);
  });

  it("draws a circle at 45 degrees of ellipticity", () => {
    const radius = 40 * Math.cos(Math.PI / 4);
    for (const [x, y] of points(ellipsePath(17, 45, 40, 48))) {
      expect(Math.hypot(x - 48, y - 48)).toBeCloseTo(radius, 1);
    }
  });

  it("draws a flat line tilted by the angle for linear polarisation", () => {
    const start = ellipsePoint(0, 90, 0, 40, 48);
    expect(start.x).toBeCloseTo(48);
    expect(start.y).toBeCloseTo(8);
    const flat = ellipsePoint(Math.PI / 2, 0, 0, 40, 48);
    expect(flat.x).toBeCloseTo(48);
    expect(flat.y).toBeCloseTo(48);
  });
});

function turn(angle: number, chi: number, v: number): number {
  const start = ellipsePoint(0, angle, chi, 40, 48);
  const next = ellipsePoint(senseStep(v, chi), angle, chi, 40, 48);
  return (start.x - 48) * (next.y - start.y) - (start.y - 48) * (next.x - start.x);
}

describe("rotation sense", () => {
  it("turns clockwise on screen for positive V and back for negative V", () => {
    for (const angle of [0, 35, 120]) {
      expect(turn(angle, 20, 0.4)).toBeGreaterThan(0);
      expect(turn(angle, -20, -0.4)).toBeLessThan(0);
      expect(turn(angle, -20, 0.4)).toBeGreaterThan(0);
      expect(turn(angle, 20, -0.4)).toBeLessThan(0);
    }
  });

  it("draws an arrowhead only for elliptical waves", () => {
    expect(senseArrow(30, 1, 0.1, 40, 48)).toBeNull();
    const arrow = senseArrow(30, 20, 0.4, 40, 48);
    expect(arrow?.head.endsWith("Z")).toBe(true);
    expect(points(arrow?.head ?? "")).toHaveLength(3);
  });
});

describe("polarimeter labels and lanes", () => {
  it("names the hand", () => {
    expect(handText("right")).toBe("RH");
    expect(handText("left")).toBe("LH");
    expect(handText(undefined)).toBe("Lin");
    expect(percentText(0.874)).toBe("87%");
    expect(percentText(1.4)).toBe("100%");
  });

  it("refuses the same lane for H and V", () => {
    const settings = defaults();
    expect(laneEdit(settings, "h_lane", settings.v_lane)).toBeNull();
    expect(laneEdit(settings, "v_lane", 3)).toEqual({ v_lane: 3 });
  });
});
