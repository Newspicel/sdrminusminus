import { type Colormap, sampleColormap } from "../gl/colormap";
import { type DbWindow, traceUnit } from "./spectrumTraces";

export const DENSITY_WIDTH = 480;
export const DENSITY_HEIGHT = 192;

export const DENSITY_DECAY = 0.92;
export const DENSITY_GAIN = 0.17;

export interface DensityGrid {
  width: number;
  height: number;
  cells: Float32Array;
  lo: Float32Array;
  hi: Float32Array;
}

export function createDensity(width = DENSITY_WIDTH, height = DENSITY_HEIGHT): DensityGrid {
  return {
    width,
    height,
    cells: new Float32Array(width * height),
    lo: new Float32Array(width),
    hi: new Float32Array(width),
  };
}

export function clearDensity(grid: DensityGrid): void {
  grid.cells.fill(0);
}

export function decayDensity(grid: DensityGrid, factor = DENSITY_DECAY): void {
  const cells = grid.cells;
  for (let i = 0; i < cells.length; i++) {
    const value = (cells[i] ?? 0) * factor;
    cells[i] = value < 0.002 ? 0 : value;
  }
}

export function addDensity(
  grid: DensityGrid,
  db: Float32Array,
  view: { start: number; end: number },
  window: DbWindow,
  gain = DENSITY_GAIN,
): void {
  const { width, height, cells, lo, hi } = grid;
  const n = db.length;
  if (n === 0 || height < 1) {
    return;
  }
  const first = view.start * (n - 1);
  const last = view.end * (n - 1);
  for (let x = 0; x < width; x++) {
    const from = first + ((last - first) * x) / width;
    const to = first + ((last - first) * (x + 1)) / width;
    const start = Math.max(0, Math.floor(from));
    const stop = Math.min(n - 1, Math.max(start, Math.floor(to)));
    let low = Number.POSITIVE_INFINITY;
    let high = Number.NEGATIVE_INFINITY;
    for (let i = start; i <= stop; i++) {
      const level = db[i] ?? Number.NEGATIVE_INFINITY;
      if (level < low) {
        low = level;
      }
      if (level > high) {
        high = level;
      }
    }
    lo[x] = low;
    hi[x] = high;
  }

  const top = height - 1;
  for (let x = 0; x < width; x++) {
    let low = lo[x] ?? Number.POSITIVE_INFINITY;
    let high = hi[x] ?? Number.NEGATIVE_INFINITY;
    if (!(high >= low)) {
      continue;
    }
    if (x > 0) {
      low = Math.min(low, hi[x - 1] ?? low);
      high = Math.max(high, lo[x - 1] ?? high);
    }
    const yLow = Math.round((1 - traceUnit(high, window)) * top);
    const yHigh = Math.round((1 - traceUnit(low, window)) * top);
    for (let y = yLow; y <= yHigh; y++) {
      const at = y * width + x;
      const value = (cells[at] ?? 0) + gain;
      cells[at] = value > 1 ? 1 : value;
    }
  }
}

export function densityToImage(
  grid: DensityGrid,
  colormap: Colormap,
  out: Uint8ClampedArray,
  lut = colormapLut(colormap),
): void {
  const cells = grid.cells;
  for (let i = 0; i < cells.length; i++) {
    const value = cells[i] ?? 0;
    const at = i * 4;
    if (value <= 0) {
      out[at + 3] = 0;
      continue;
    }
    const entry = (Math.min(255, Math.round(value * 255)) | 0) * 3;
    out[at] = lut[entry] ?? 0;
    out[at + 1] = lut[entry + 1] ?? 0;
    out[at + 2] = lut[entry + 2] ?? 0;
    out[at + 3] = Math.min(255, Math.round(10 + value * 245));
  }
}

const luts = new Map<Colormap, Uint8Array>();

export function colormapLut(map: Colormap): Uint8Array {
  const cached = luts.get(map);
  if (cached !== undefined) {
    return cached;
  }
  const lut = new Uint8Array(256 * 3);
  for (let i = 0; i < 256; i++) {
    const [r, g, b] = sampleColormap(map, i / 255);
    lut[i * 3] = Math.round(r * 255);
    lut[i * 3 + 1] = Math.round(g * 255);
    lut[i * 3 + 2] = Math.round(b * 255);
  }
  luts.set(map, lut);
  return lut;
}
