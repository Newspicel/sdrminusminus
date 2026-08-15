import { describe, expect, it } from "vitest";
import {
  addDensity,
  clearDensity,
  colormapLut,
  createDensity,
  DENSITY_GAIN,
  decayDensity,
  densityToImage,
} from "./persistence";

const FULL = { start: 0, end: 1 };
const WINDOW = { min: -100, max: -20 };

function painted(
  grid: ReturnType<typeof createDensity>,
  column: number,
): { row: number; value: number }[] {
  const hits: { row: number; value: number }[] = [];
  for (let y = 0; y < grid.height; y++) {
    const value = grid.cells[y * grid.width + column] ?? 0;
    if (value > 0) {
      hits.push({ row: y, value });
    }
  }
  return hits;
}

describe("addDensity", () => {
  it("paints a flat trace at the level's own row", () => {
    const grid = createDensity(4, 101);
    addDensity(grid, Float32Array.of(-60, -60, -60, -60), FULL, WINDOW);

    for (let x = 0; x < grid.width; x++) {
      expect(painted(grid, x).map((hit) => hit.row)).toEqual([50]);
    }
  });

  it("joins a steep edge into a continuous segment", () => {
    const grid = createDensity(2, 101);
    addDensity(grid, Float32Array.of(-100, -20), FULL, WINDOW);

    const rows = painted(grid, 1).map((hit) => hit.row);
    expect(rows[0]).toBe(0);
    expect(rows.at(-1)).toBe(100);
    expect(rows.length).toBe(101);
  });

  it("clamps levels above and below the window onto its edges", () => {
    const grid = createDensity(1, 101);
    addDensity(grid, Float32Array.of(40), FULL, WINDOW);
    expect(painted(grid, 0).map((hit) => hit.row)).toEqual([0]);

    clearDensity(grid);
    addDensity(grid, Float32Array.of(-500), FULL, WINDOW);
    expect(painted(grid, 0).map((hit) => hit.row)).toEqual([100]);
  });

  it("follows the visible window when the plot is zoomed", () => {
    const grid = createDensity(2, 101);
    const db = Float32Array.of(-100, -100, -100, -20, -20, -20);
    addDensity(grid, db, { start: 0.6, end: 1 }, WINDOW);
    expect(painted(grid, 0).map((hit) => hit.row)).toEqual([0]);
    expect(painted(grid, 1).map((hit) => hit.row)).toEqual([0]);
  });

  it("saturates a repeated visit instead of running past full brightness", () => {
    const grid = createDensity(1, 3);
    for (let i = 0; i < 20; i++) {
      addDensity(grid, Float32Array.of(-20), FULL, WINDOW);
    }
    expect(grid.cells[0]).toBe(1);
  });

  it("accumulates one gain step per visit", () => {
    const grid = createDensity(1, 3);
    addDensity(grid, Float32Array.of(-20), FULL, WINDOW);
    expect(grid.cells[0]).toBeCloseTo(DENSITY_GAIN, 6);
  });

  it("leaves the grid alone when there are no bins", () => {
    const grid = createDensity(2, 3);
    addDensity(grid, new Float32Array(0), FULL, WINDOW);
    expect(grid.cells.some((cell) => cell > 0)).toBe(false);
  });
});

describe("decayDensity", () => {
  it("fades a cell towards zero and snaps it there", () => {
    const grid = createDensity(1, 1);
    addDensity(grid, Float32Array.of(-20), FULL, WINDOW);
    decayDensity(grid, 0.5);
    expect(grid.cells[0]).toBeCloseTo(DENSITY_GAIN / 2, 6);

    for (let i = 0; i < 20; i++) {
      decayDensity(grid, 0.5);
    }
    expect(grid.cells[0]).toBe(0);
  });
});

describe("densityToImage", () => {
  it("leaves untouched cells fully transparent", () => {
    const grid = createDensity(2, 1);
    const out = new Uint8ClampedArray(grid.width * grid.height * 4);
    densityToImage(grid, "gray", out);
    expect(out[3]).toBe(0);
    expect(out[7]).toBe(0);
  });

  it("brightens and opacifies with density", () => {
    const grid = createDensity(2, 1);
    grid.cells[0] = 0.25;
    grid.cells[1] = 1;
    const out = new Uint8ClampedArray(grid.width * grid.height * 4);
    densityToImage(grid, "gray", out);

    expect(out[0]).toBeLessThan(out[4] ?? 0);
    expect(out[3]).toBeLessThan(out[7] ?? 0);
    expect(out[7]).toBe(255);
  });
});

describe("colormapLut", () => {
  it("builds 256 triples and caches them", () => {
    const lut = colormapLut("viridis");
    expect(lut).toHaveLength(768);
    expect(colormapLut("viridis")).toBe(lut);
  });

  it("matches the ramp's ends", () => {
    const lut = colormapLut("gray");
    expect([lut[0], lut[1], lut[2]]).toEqual([0, 0, 0]);
    expect([lut[765], lut[766], lut[767]]).toEqual([255, 255, 255]);
  });
});
