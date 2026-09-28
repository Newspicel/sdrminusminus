import { describe, expect, it } from "vitest";
import { gridPixels, paletteTable } from "../gl/surface";

const TABLE = paletteTable("gray");

function frame(cells: number[], cols: number) {
  return { cols, rows: cells.length / cols, cells: Uint8Array.from(cells) };
}

function reds(pixels: Uint8ClampedArray): number[] {
  return [...pixels].filter((_, index) => index % 4 === 0);
}

describe("gridPixels", () => {
  it("flips rows when asked", () => {
    const grid = frame([0, 0, 255, 255], 2);
    expect(reds(gridPixels(grid, TABLE, { flipY: false, transparentFloor: false }))).toEqual([
      0, 0, 255, 255,
    ]);
    expect(reds(gridPixels(grid, TABLE, { flipY: true, transparentFloor: false }))).toEqual([
      255, 255, 0, 0,
    ]);
  });

  it("makes the floor transparent", () => {
    const grid = frame([0, 255], 2);
    const opaque = gridPixels(grid, TABLE, { flipY: false, transparentFloor: false });
    expect([opaque[3], opaque[7]]).toEqual([255, 255]);
    const see = gridPixels(grid, TABLE, { flipY: false, transparentFloor: true });
    expect(see[3]).toBe(0);
    expect(see[7]).toBe(Math.round(255 * 0.85));
  });

  it("reuses the output buffer", () => {
    const grid = frame([10, 20, 30, 40], 2);
    const out = new Uint8ClampedArray(16);
    expect(gridPixels(grid, TABLE, { flipY: false, transparentFloor: false }, out)).toBe(out);
    const wrong = new Uint8ClampedArray(8);
    expect(gridPixels(grid, TABLE, { flipY: false, transparentFloor: false }, wrong)).not.toBe(
      wrong,
    );
  });
});
