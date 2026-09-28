import { describe, expect, it } from "vitest";
import { linearTicks } from "../components/plotFrame";
import { floorAlpha, gridCellAt, PALETTE_STEPS, paletteTable, validGrid } from "./surface";

describe("gridCellAt", () => {
  it("maps pixels back to the cell drawn there", () => {
    const box = { w: 200, h: 100 };
    const frame = { cols: 20, rows: 10 };
    expect(gridCellAt(0, 0, box, frame, false)).toEqual({ col: 0, row: 0 });
    expect(gridCellAt(199.9, 99.9, box, frame, false)).toEqual({ col: 19, row: 9 });
    expect(gridCellAt(105, 55, box, frame, false)).toEqual({ col: 10, row: 5 });
  });

  it("counts rows from the bottom when drawn upside down", () => {
    const box = { w: 200, h: 100 };
    const frame = { cols: 20, rows: 10 };
    expect(gridCellAt(0, 0, box, frame, true)).toEqual({ col: 0, row: 9 });
    expect(gridCellAt(0, 99, box, frame, true)).toEqual({ col: 0, row: 0 });
  });

  it("finds nothing outside the plot or in an empty frame", () => {
    const frame = { cols: 20, rows: 10 };
    expect(gridCellAt(-1, 5, { w: 200, h: 100 }, frame, false)).toBeNull();
    expect(gridCellAt(200, 5, { w: 200, h: 100 }, frame, false)).toBeNull();
    expect(gridCellAt(5, 5, { w: 0, h: 100 }, frame, false)).toBeNull();
    expect(gridCellAt(5, 5, { w: 200, h: 100 }, { cols: 0, rows: 10 }, false)).toBeNull();
  });
});

describe("grid painter", () => {
  it("keeps axis ticks on round numbers", () => {
    const ticks = linearTicks(-0.075, 14.925, 5, (km) => km * 10);
    expect(ticks.map((tick) => tick.label)).toEqual(["0", "5", "10"]);
    expect(ticks.map((tick) => tick.px)).toEqual([0, 50, 100]);
    expect(linearTicks(-65, 65, 5, (hz) => hz).map((tick) => tick.label)).toEqual([
      "-50",
      "0",
      "50",
    ]);
    expect(linearTicks(0, 0, 5, (hz) => hz)).toEqual([]);
  });

  it("builds one colour per level", () => {
    const table = paletteTable("gray");
    expect(table).toHaveLength(PALETTE_STEPS * 3);
    expect(Array.from(table.slice(0, 3))).toEqual([0, 0, 0]);
    expect(Array.from(table.slice(-3))).toEqual([255, 255, 255]);
  });

  it("fades the floor in towards full level", () => {
    expect(floorAlpha(0)).toBe(0);
    expect(floorAlpha(255)).toBe(Math.round(255 * 0.85));
    expect(floorAlpha(64)).toBeLessThan(floorAlpha(128));
  });

  it("refuses a frame whose cells do not fill it", () => {
    expect(validGrid({ cols: 2, rows: 2, cells: new Uint8Array(4) })).toBe(true);
    expect(validGrid({ cols: 2, rows: 2, cells: new Uint8Array(3) })).toBe(false);
    expect(validGrid({ cols: 0, rows: 2, cells: new Uint8Array(0) })).toBe(false);
  });
});
