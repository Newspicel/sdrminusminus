import type { Map as MapLibreMap } from "maplibre-gl";
import { describe, expect, it, vi } from "vitest";
import { paletteTable } from "../../gl/surface";
import type { FusionGridFrame } from "../frame";
import {
  HEAT_BELOW,
  HEAT_LAYER,
  HEAT_SOURCE,
  heatCoordinates,
  heatPixels,
  installHeatLayer,
} from "./heat";

function grid(cells: number[], cols: number): FusionGridFrame {
  return {
    streamId: 1,
    seq: 1,
    timestamp: 0n,
    south: 51.9,
    west: 12.9,
    north: 52.1,
    east: 13.2,
    cols,
    rows: cells.length / cols,
    cells: Uint8Array.from(cells),
  };
}

describe("heatCoordinates", () => {
  it("orders the corners clockwise from the north-west", () => {
    expect(heatCoordinates(grid([0], 1))).toEqual([
      [12.9, 52.1],
      [13.2, 52.1],
      [13.2, 51.9],
      [12.9, 51.9],
    ]);
  });
});

describe("heatPixels", () => {
  it("keeps the north row first, hides the floor and reuses its buffer", () => {
    const scratch = { pixels: null };
    const table = paletteTable("gray");
    const pixels = heatPixels(grid([255, 0, 0, 0], 2), table, scratch);
    expect(Array.from(pixels.slice(0, 4))).toEqual([255, 255, 255, Math.round(255 * 0.85)]);
    expect(pixels[7]).toBe(0);
    expect(scratch.pixels).toBe(pixels);
    expect(heatPixels(grid([1, 2, 3, 4], 2), table, scratch)).toBe(pixels);
  });
});

describe("installHeatLayer", () => {
  it("adds an image source under the bearing rays", () => {
    const layers = new Set(["df-rays"]);
    const map = {
      getLayer: (id: string) => (layers.has(id) ? { id } : undefined),
      getSource: () => undefined,
      removeLayer: vi.fn(),
      removeSource: vi.fn(),
      addSource: vi.fn(),
      addLayer: vi.fn(),
    };
    installHeatLayer(map as unknown as MapLibreMap, true);
    expect(map.addSource).toHaveBeenCalledWith(
      HEAT_SOURCE,
      expect.objectContaining({ type: "image" }),
    );
    expect(map.addLayer).toHaveBeenCalledWith(
      expect.objectContaining({ id: HEAT_LAYER, type: "raster" }),
      HEAT_BELOW,
    );
    const off = { ...map, addSource: vi.fn(), addLayer: vi.fn() };
    installHeatLayer(off as unknown as MapLibreMap, false);
    expect(off.addLayer).not.toHaveBeenCalled();
  });
});
