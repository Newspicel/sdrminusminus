// The persistence ("digital phosphor") display: a decaying 2D histogram of level against
// frequency, so how *often* a bin sits at a level becomes visible alongside where it sits now.
//
// It is what separates a steady carrier from a noise excursion that happens to peak as high, and
// it shows modulation structure — skirts, shoulders, the two humps of an FSK pair — that a live
// trace only flickers through.
//
// The grid is deliberately small and fixed: it is scaled up to the plot when drawn, and decaying
// one cell per plot pixel every animation frame would cost more than the display is worth.

import { type Colormap, sampleColormap } from "../gl/colormap";
import { type DbWindow, traceUnit } from "./spectrumTraces";

/** Columns of the density grid. Wider than most scope nodes are drawn, so the upscale is mild. */
export const DENSITY_WIDTH = 480;
/** Level rows. 192 over a 90 dB window is a shade under half a dB per row. */
export const DENSITY_HEIGHT = 192;

/** Per-frame multiplier. At 30 fps a cell left alone fades out of sight in about a second. */
export const DENSITY_DECAY = 0.92;
/** What one frame's visit to a cell adds. A cell the trace really dwells in saturates in about
 * six frames; a single excursion stays faint, which is the whole distinction being drawn. */
export const DENSITY_GAIN = 0.17;

export interface DensityGrid {
  width: number;
  height: number;
  /** `width × height`, row 0 at the top of the plot (the window's ceiling). */
  cells: Float32Array;
  /** Per-column dB extent of the frame being folded in. Kept here so `addDensity` allocates
   * nothing on the steady path. */
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

/** Fade every cell by one frame's worth. */
export function decayDensity(grid: DensityGrid, factor = DENSITY_DECAY): void {
  const cells = grid.cells;
  for (let i = 0; i < cells.length; i++) {
    const value = (cells[i] ?? 0) * factor;
    // Snapped to zero rather than left to denormals: a grid nobody is feeding should stop costing
    // arithmetic, and a cell below this is already the darkest colour the ramp has.
    cells[i] = value < 0.002 ? 0 : value;
  }
}

/**
 * Paint one frame's levels into the grid.
 *
 * Each column takes the dB extent of every bin that falls in it, and the segment drawn is that
 * extent joined to the previous column's — a trace painted as isolated points breaks into dots
 * wherever it is steep, which is exactly where the interesting signals are.
 *
 * `view` is the visible fraction of the device span, so the display follows a zoom the way the
 * trace above it does.
 */
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
      // Join to the neighbour so a steep edge is a line and not a ladder of dots.
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

/**
 * Colour the grid into RGBA bytes for `putImageData`. Cells at zero come out fully transparent so
 * the plot's own ground and grid show through where nothing has been drawn.
 */
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
    // Faint cells stay translucent as well as dark. The floor is deliberately low: noise wanders
    // over a wide band of levels, and a visible floor turns that wander into a speckled slab that
    // hides the very dwell the display exists to show.
    out[at + 3] = Math.min(255, Math.round(10 + value * 245));
  }
}

const luts = new Map<Colormap, Uint8Array>();

/** 256 RGB triples for a ramp, built once. Sampling the polynomial per cell would cost twenty
 * multiplies a pixel for a value that only ever takes 256 distinct inputs. */
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
