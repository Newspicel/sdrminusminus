export const BASEBAND_GAIN = 0.22;
export const BASEBAND_DECAY = 0.82;

export interface BasebandGrid {
  width: number;
  height: number;
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
    const x = Math.round(halfW + Math.min(1, Math.max(-1, re)) * halfW);
    const y = Math.round(halfH - Math.min(1, Math.max(-1, im)) * halfH);
    paint(grid, x, y, gain);
  }
}

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

function rail(samples: Float32Array, i: number, component: EyeComponent): number {
  const re = samples[i * 2] ?? 0;
  const im = samples[i * 2 + 1] ?? 0;
  if (component === "i") {
    return re;
  }
  if (component === "q") {
    return im;
  }
  if (i === 0) {
    return 0;
  }
  const pr = samples[i * 2 - 2] ?? 0;
  const pi = samples[i * 2 - 1] ?? 0;
  return Math.atan2(im * pr - re * pi, re * pr + im * pi) / Math.PI;
}

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

export function samplesPerSymbol(sampleRate: number, symbolRate: number): number {
  if (!(sampleRate > 0) || !(symbolRate > 0)) {
    return 1;
  }
  return sampleRate / symbolRate;
}
