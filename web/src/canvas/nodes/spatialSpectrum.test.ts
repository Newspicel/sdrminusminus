import { describe, expect, it } from "vitest";
import fixtures from "../../generated/frame-fixtures.json";
import { hueRgb, trailRow } from "../../gl/spatial";
import { decodeSpatialSpectrum, type SpatialSpectrumFrame } from "../../lib/frame";
import {
  belowPeakDb,
  binHz,
  columnArgmax,
  cursorText,
  hoverAt,
  peakText,
  rotateRows,
  rowDeg,
  rowShift,
  topPeaks,
} from "./spatialSpectrum";

function frame(bearings: number, bins: number, cells: number[]): SpatialSpectrumFrame {
  return {
    streamId: 1,
    seq: 1,
    timestamp: 0n,
    centerHz: 433_920_000,
    spanHz: 1_000_000,
    bearings,
    bins,
    dbMin: -80,
    dbMax: -20,
    cells: Uint8Array.from(cells),
  };
}

function fixture(name: string): ArrayBuffer {
  const found = (fixtures as [string, number[]][]).find(([kind]) => kind === name);
  if (found === undefined) {
    throw new Error(`the fixtures have no ${name} frame`);
  }
  return Uint8Array.from(found[1]).buffer;
}

describe("spatial spectrum axes", () => {
  it("maps a bin to its centre frequency and a row to its bearing", () => {
    expect(binHz(0, 4, 100e6, 4e6)).toBe(98.5e6);
    expect(binHz(3, 4, 100e6, 4e6)).toBe(101.5e6);
    expect(rowDeg(0, 180)).toBe(0);
    expect(rowDeg(45, 180)).toBe(90);
  });

  it("rotates rows by the array azimuth", () => {
    const grid = frame(4, 2, [1, 1, 2, 2, 3, 3, 4, 4]);
    expect([...rotateRows(grid, 90)]).toEqual([4, 4, 1, 1, 2, 2, 3, 3]);
    expect([...rotateRows(grid, -90)]).toEqual([2, 2, 3, 3, 4, 4, 1, 1]);
    expect([...rotateRows(grid, 360)]).toEqual([...grid.cells]);
    expect(rowShift(450, 4)).toBe(1);
    const out = new Uint8Array(8);
    expect(rotateRows(grid, 0, out)).toBe(out);
  });

  it("reads the cursor as frequency, bearing and level", () => {
    const grid = frame(
      4,
      4,
      Array.from({ length: 16 }, () => 255),
    );
    const at = hoverAt(50, 25, { w: 100, h: 100 }, grid);
    expect(at).toMatchObject({ col: 2, row: 1, deg: 90 });
    expect(cursorText(at.hz, at.deg, -62.4)).toBe("434.045 MHz 90° -62 dB");
    expect(belowPeakDb(255, grid)).toBe(0);
    expect(belowPeakDb(0, grid)).toBe(-60);
  });
});

describe("spatial spectrum peaks", () => {
  it("finds the strongest bearing of each column", () => {
    const grid = frame(3, 2, [1, 9, 7, 2, 3, 4]);
    expect(columnArgmax(grid.cells, 3, 2, 0)).toEqual({ row: 1, level: 7 });
    expect(columnArgmax(grid.cells, 3, 2, 1)).toEqual({ row: 0, level: 9 });
  });

  it("lists the three strongest peaks in the chosen frame", () => {
    const reading = {
      at: "now",
      peaks: [
        { freq_hz: 433_920_000, bearing_deg: 137, db: -60, true_deg: 227 },
        { freq_hz: 145_000_000, bearing_deg: 10, db: -40 },
        { freq_hz: 1e6, bearing_deg: 0, db: -90 },
        { freq_hz: 2e6, bearing_deg: 0, db: -95 },
      ],
    };
    const peaks = topPeaks(reading);
    expect(peaks.map((peak) => peak.db)).toEqual([-40, -60, -90]);
    const second = peaks[1];
    expect(second === undefined ? "" : peakText(second, "true")).toBe("433.920 MHz 227°");
    expect(second === undefined ? "" : peakText(second, "relative")).toBe("433.920 MHz 137°");
    const first = peaks[0];
    expect(first === undefined ? "" : peakText(first, "true", 90)).toBe("145.000 MHz 100°");
  });

  it("colours the trail by bearing and brightness by power", () => {
    expect(hueRgb(0, 1)).toEqual([255, 38, 38]);
    expect(hueRgb(120, 0)).toEqual([0, 0, 0]);
    const out = new Uint8ClampedArray(8);
    trailRow(frame(2, 2, [255, 0, 0, 0]), 0, out);
    expect(Array.from(out.slice(0, 4))).toEqual([255, 38, 38, 255]);
    expect(out[4]).toBe(0);
  });
});

describe("spatial spectrum frames", () => {
  it("decodes the generated fixture", () => {
    const decoded = decodeSpatialSpectrum(fixture("spatial_spectrum"));
    expect(decoded).not.toBeNull();
    expect(decoded?.cells.length).toBe((decoded?.bearings ?? 0) * (decoded?.bins ?? 0));
  });
});
