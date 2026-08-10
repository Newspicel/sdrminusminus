import { describe, expect, it } from "vitest";
import { backingPx, fitExtent, nextRingRow, pixelRatio, rowsForHeight, zoomOf } from "./raster";

describe("zoomOf", () => {
  it("is the ratio between drawn and laid-out size", () => {
    expect(zoomOf(300, 150)).toBe(2);
    expect(zoomOf(75, 150)).toBe(0.5);
  });

  it("falls back to 1 for a node that has not been laid out", () => {
    expect(zoomOf(0, 0)).toBe(1);
    expect(zoomOf(200, 0)).toBe(1);
  });
});

describe("pixelRatio", () => {
  it("follows the display when the canvas is not zoomed", () => {
    expect(pixelRatio(1, 1)).toBe(1);
    expect(pixelRatio(2, 1)).toBe(2);
  });

  it("clamps the display ratio before zoom multiplies it", () => {
    expect(pixelRatio(3, 1)).toBe(2);
  });

  it("re-renders a zoomed plot rather than stretching it", () => {
    expect(pixelRatio(1, 1.5)).toBe(1.5);
    expect(pixelRatio(1, 0.5)).toBe(0.5);
  });

  it("caps the product so a zoomed retina plot stays affordable", () => {
    expect(pixelRatio(2, 2)).toBe(3);
  });

  it("keeps a plot zoomed far out above the floor", () => {
    expect(pixelRatio(1, 0.1)).toBe(0.5);
  });

  it("snaps to eighths, so a zoom gesture resizes buffers a handful of times", () => {
    const ratios = new Set<number>();
    for (let zoom = 1; zoom <= 2; zoom += 0.005) {
      ratios.add(pixelRatio(1, zoom));
    }
    expect(ratios.size).toBeLessThanOrEqual(9);
  });

  it("treats a missing or nonsense ratio as 1", () => {
    expect(pixelRatio(Number.NaN, 1)).toBe(1);
    expect(pixelRatio(0, 1)).toBe(1);
    expect(pixelRatio(1, Number.NaN)).toBe(1);
  });
});

describe("backingPx", () => {
  it("rounds to whole device pixels", () => {
    expect(backingPx(100, 1.5)).toBe(150);
    expect(backingPx(101, 1.5)).toBe(152);
  });

  it("is zero for a plot laid out at zero, which must not be drawn", () => {
    expect(backingPx(0, 2)).toBe(0);
  });
});

describe("rowsForHeight", () => {
  it("shows one history row per layout pixel, whatever the device ratio", () => {
    expect(rowsForHeight(600, 2, 1024)).toBe(300);
    expect(rowsForHeight(300, 1, 1024)).toBe(300);
  });

  it("never asks for more rows than the ring holds", () => {
    expect(rowsForHeight(4096, 1, 1024)).toBe(1024);
  });

  it("never collapses to a single row, which the shader divides by", () => {
    expect(rowsForHeight(1, 2, 1024)).toBe(2);
  });
});

describe("nextRingRow", () => {
  it("wraps at the top of the ring", () => {
    expect(nextRingRow(0, 4)).toBe(1);
    expect(nextRingRow(3, 4)).toBe(0);
  });

  it("visits every row exactly once per lap", () => {
    const seen = new Set<number>();
    let row = 0;
    for (let i = 0; i < 8; i++) {
      seen.add(row);
      row = nextRingRow(row, 8);
    }
    expect(seen.size).toBe(8);
    expect(row).toBe(0);
  });
});

describe("fitExtent", () => {
  it("grows to the largest plot on screen at once", () => {
    expect(fitExtent(512, 900)).toBe(900);
  });

  it("holds its size through a small shrink", () => {
    expect(fitExtent(900, 600)).toBe(900);
  });

  it("gives the memory back once half of it is idle", () => {
    expect(fitExtent(900, 400)).toBe(400);
  });

  it("stays allocatable when nothing is visible", () => {
    expect(fitExtent(900, 0)).toBe(1);
  });
});
