// The three ways of looking at a channel's baseband, as pure grids the face then paints.
//
// All three accumulate into a decaying density grid rather than drawing points: a burst is two
// thousand samples twenty times a second, and what an operator needs to see is where the signal
// *dwells* — the tight knots of a locked constellation against the smear of one that is not, the
// open centre of a clean eye against a closed one. Isolated dots answer neither question.

/** Levels a density cell can hold before it saturates; one visit adds `gain`. */
export const BASEBAND_GAIN = 0.22;
/** Per-burst fade. Slower than the spectrum phosphor: bursts arrive with gaps between them, and
 * the shape being read is built from several. */
export const BASEBAND_DECAY = 0.82;

export interface BasebandGrid {
  width: number;
  height: number;
  /** `width × height`, row 0 at the top. */
  cells: Float32Array;
}

export function createBasebandGrid(width: number, height: number): BasebandGrid {
  return { width, height, cells: new Float32Array(width * height) };
}

export function clearBasebandGrid(grid: BasebandGrid): void {
  grid.cells.fill(0);
}

export function decayBasebandGrid(grid: BasebandGrid, factor = BASEBAND_DECAY): void {
  const cells = grid.cells;
  for (let i = 0; i < cells.length; i++) {
    const value = (cells[i] ?? 0) * factor;
    cells[i] = value < 0.002 ? 0 : value;
  }
}

function paint(grid: BasebandGrid, x: number, y: number, gain: number): void {
  if (x < 0 || y < 0 || x >= grid.width || y >= grid.height) {
    return;
  }
  const at = y * grid.width + x;
  const value = (grid.cells[at] ?? 0) + gain;
  grid.cells[at] = value > 1 ? 1 : value;
}

/**
 * Largest magnitude in a burst, which is what a constellation is scaled against.
 *
 * Auto-scaled rather than fixed to full scale: a channel's baseband sits wherever the gain and
 * the filter leave it, and a constellation drawn at absolute scale is a dot in the middle of an
 * empty square nearly every time.
 */
export function peakMagnitude(samples: Float32Array): number {
  let peak = 0;
  for (let i = 0; i + 1 < samples.length; i += 2) {
    const magnitude = Math.hypot(samples[i] ?? 0, samples[i + 1] ?? 0);
    if (magnitude > peak) {
      peak = magnitude;
    }
  }
  return peak;
}

/**
 * Plot I against Q.
 *
 * `scale` is the magnitude that reaches the edge of the square; samples beyond it clamp there
 * rather than vanishing, so a transient never silently empties the plot.
 *
 * `decimation` plots every nth sample. At 1 the whole burst is drawn, which for a signal with
 * several samples per symbol paints the *transitions* as well as the symbols — the trajectory,
 * which is what shows filter overshoot. Set it to the samples per symbol and only the decision
 * points remain, which is the classic constellation.
 */
export function addConstellation(
  grid: BasebandGrid,
  samples: Float32Array,
  scale: number,
  decimation = 1,
  offset = 0,
  gain = BASEBAND_GAIN,
): void {
  const span = scale > 0 ? scale : 1;
  const halfW = (grid.width - 1) / 2;
  const halfH = (grid.height - 1) / 2;
  const step = Math.max(1, Math.floor(decimation));
  const start = ((offset % step) + step) % step;
  for (let i = start; i * 2 + 1 < samples.length; i += step) {
    const re = (samples[i * 2] ?? 0) / span;
    const im = (samples[i * 2 + 1] ?? 0) / span;
    // Q upward: the plot is an Argand diagram, not an image raster.
    const x = Math.round(halfW + Math.min(1, Math.max(-1, re)) * halfW);
    const y = Math.round(halfH - Math.min(1, Math.max(-1, im)) * halfH);
    paint(grid, x, y, gain);
  }
}

/**
 * Fold the burst into `period` sample windows and overlay them, which is an eye diagram.
 *
 * `component` picks what is folded: the in-phase or quadrature rail, or the instantaneous
 * frequency — the last is the one that opens for FSK, where I and Q individually show nothing but
 * a rotating phasor.
 *
 * Two periods wide, as an eye is conventionally drawn: one period shows the opening but cuts the
 * transitions either side of it in half.
 */
export function addEye(
  grid: BasebandGrid,
  samples: Float32Array,
  period: number,
  component: EyeComponent,
  scale: number,
  gain = BASEBAND_GAIN,
): void {
  const span = scale > 0 ? scale : 1;
  const count = samples.length >> 1;
  const width = Math.max(2, Math.round(period)) * 2;
  if (count < width) {
    return;
  }
  const halfH = (grid.height - 1) / 2;
  for (let start = 0; start + width <= count; start += width >> 1) {
    for (let k = 0; k < width; k++) {
      const value = rail(samples, start + k, component) / span;
      const x = Math.round((k / (width - 1)) * (grid.width - 1));
      const y = Math.round(halfH - Math.min(1, Math.max(-1, value)) * halfH);
      paint(grid, x, y, gain);
    }
  }
}

export const EYE_COMPONENTS = ["i", "q", "frequency"] as const;
export type EyeComponent = (typeof EYE_COMPONENTS)[number];

/** One rail's value at complex sample `i`. */
function rail(samples: Float32Array, i: number, component: EyeComponent): number {
  const re = samples[i * 2] ?? 0;
  const im = samples[i * 2 + 1] ?? 0;
  if (component === "i") {
    return re;
  }
  if (component === "q") {
    return im;
  }
  // Instantaneous frequency: the phase advance since the previous sample, which is what an FSK
  // eye is drawn from. The first sample has no predecessor and reads zero.
  if (i === 0) {
    return 0;
  }
  const pr = samples[i * 2 - 2] ?? 0;
  const pi = samples[i * 2 - 1] ?? 0;
  // The product with the conjugate of the previous sample; its argument is the phase step.
  return Math.atan2(im * pr - re * pi, re * pr + im * pi) / Math.PI;
}

/**
 * A scale for the eye's vertical axis: the largest value the chosen rail reaches.
 *
 * Frequency is already normalized to ±1 by construction, so it is left alone — auto-scaling it
 * would stretch a narrow-deviation signal until its noise filled the plot.
 */
export function eyeScale(samples: Float32Array, component: EyeComponent): number {
  if (component === "frequency") {
    return 1;
  }
  let peak = 0;
  const count = samples.length >> 1;
  for (let i = 0; i < count; i++) {
    const value = Math.abs(rail(samples, i, component));
    if (value > peak) {
      peak = value;
    }
  }
  return peak;
}

/** Samples per symbol implied by a symbol rate, which is how the face turns a rate the operator
 * knows into the period these grids fold on. */
export function samplesPerSymbol(sampleRate: number, symbolRate: number): number {
  if (!(sampleRate > 0) || !(symbolRate > 0)) {
    return 1;
  }
  return sampleRate / symbolRate;
}
