import { describe, expect, it } from "vitest";
import {
  addConstellation,
  addEye,
  BASEBAND_GAIN,
  clearBasebandGrid,
  createBasebandGrid,
  decayBasebandGrid,
  eyeScale,
  peakMagnitude,
  samplesPerSymbol,
  symbolHistogram,
  symbolPhase,
  Trend,
} from "./baseband";

function lit(grid: ReturnType<typeof createBasebandGrid>): { x: number; y: number }[] {
  const hits: { x: number; y: number }[] = [];
  for (let y = 0; y < grid.height; y++) {
    for (let x = 0; x < grid.width; x++) {
      if ((grid.cells[y * grid.width + x] ?? 0) > 0) {
        hits.push({ x, y });
      }
    }
  }
  return hits;
}

function iq(...pairs: [number, number][]): Float32Array {
  return Float32Array.from(pairs.flat());
}

describe("peakMagnitude", () => {
  it("is the largest magnitude in the burst", () => {
    expect(peakMagnitude(iq([3, 4], [1, 0]))).toBe(5);
    expect(peakMagnitude(new Float32Array(0))).toBe(0);
  });

  it("ignores a trailing component with no partner", () => {
    expect(peakMagnitude(Float32Array.from([0, 1, 9]))).toBe(1);
  });
});

describe("addConstellation", () => {
  it("puts the origin in the middle and +I to the right", () => {
    const grid = createBasebandGrid(11, 11);
    addConstellation(grid, iq([0, 0], [1, 0]), 1);
    expect(lit(grid)).toEqual([
      { x: 5, y: 5 },
      { x: 10, y: 5 },
    ]);
  });

  it("draws +Q upward, not downward", () => {
    const grid = createBasebandGrid(11, 11);
    addConstellation(grid, iq([0, 1]), 1);
    expect(lit(grid)).toEqual([{ x: 5, y: 0 }]);
  });

  it("clamps a sample past the scale onto the edge rather than dropping it", () => {
    const grid = createBasebandGrid(11, 11);
    addConstellation(grid, iq([50, 0]), 1);
    expect(lit(grid)).toEqual([{ x: 10, y: 5 }]);
  });

  it("plots only every nth sample when decimating to the symbol points", () => {
    const grid = createBasebandGrid(11, 11);
    addConstellation(grid, iq([1, 0], [0, 0], [-1, 0], [0, 0]), 1, 2);
    expect(lit(grid)).toEqual([
      { x: 0, y: 5 },
      { x: 10, y: 5 },
    ]);
  });

  it("shifts which sample a decimated plot lands on", () => {
    const grid = createBasebandGrid(11, 11);
    addConstellation(grid, iq([1, 0], [0, 0], [-1, 0], [0, 0]), 1, 2, 1);
    expect(lit(grid)).toEqual([{ x: 5, y: 5 }]);
  });

  it("accumulates one gain step per visit and saturates", () => {
    const grid = createBasebandGrid(3, 3);
    addConstellation(grid, iq([0, 0]), 1);
    expect(grid.cells[4]).toBeCloseTo(BASEBAND_GAIN, 6);
    for (let i = 0; i < 40; i++) {
      addConstellation(grid, iq([0, 0]), 1);
    }
    expect(grid.cells[4]).toBe(1);
  });

  it("survives a zero scale rather than dividing by it", () => {
    const grid = createBasebandGrid(5, 5);
    addConstellation(grid, iq([0, 0]), 0);
    expect(lit(grid)).toEqual([{ x: 2, y: 2 }]);
  });
});

describe("addEye", () => {
  it("overlays every window on the same two-period span", () => {
    const grid = createBasebandGrid(9, 9);
    const samples = iq([1, 0], [1, 0], [-1, 0], [-1, 0], [1, 0], [1, 0], [-1, 0], [-1, 0]);
    addEye(grid, samples, 2, "i", 1);

    const columns = new Set(lit(grid).map((hit) => hit.x));
    expect(columns).toEqual(new Set([0, 3, 5, 8]));
    expect(new Set(lit(grid).map((hit) => hit.y))).toEqual(new Set([0, 8]));
  });

  it("does nothing with a burst shorter than one window", () => {
    const grid = createBasebandGrid(9, 9);
    addEye(grid, iq([1, 0], [1, 0]), 8, "i", 1);
    expect(lit(grid)).toEqual([]);
  });

  it("folds the quadrature rail when asked for it", () => {
    const grid = createBasebandGrid(9, 9);
    const samples = iq([0, 1], [0, 1], [0, -1], [0, -1]);
    addEye(grid, samples, 2, "q", 1);
    expect(new Set(lit(grid).map((hit) => hit.y))).toEqual(new Set([0, 8]));
  });

  it("reads a rotating phasor as a steady frequency", () => {
    const pairs: [number, number][] = [];
    for (let i = 0; i < 16; i++) {
      pairs.push([Math.cos((i * Math.PI) / 2), Math.sin((i * Math.PI) / 2)]);
    }
    const grid = createBasebandGrid(9, 9);
    addEye(grid, iq(...pairs), 2, "frequency", 1);

    const rows = new Set(lit(grid).map((hit) => hit.y));
    expect(rows.has(2)).toBe(true);
    expect([...rows].every((row) => row === 2 || row === 4)).toBe(true);
  });
});

describe("eyeScale", () => {
  it("auto-scales the I and Q rails to their own peak", () => {
    expect(eyeScale(iq([0.25, 0], [-0.5, 0]), "i")).toBeCloseTo(0.5, 6);
    expect(eyeScale(iq([0, 0.25], [0, -0.125]), "q")).toBeCloseTo(0.25, 6);
  });

  it("leaves frequency at full scale, which it already is", () => {
    expect(eyeScale(iq([0.001, 0.001]), "frequency")).toBe(1);
  });
});

describe("decayBasebandGrid", () => {
  it("fades towards zero and snaps there", () => {
    const grid = createBasebandGrid(1, 1);
    addConstellation(grid, iq([0, 0]), 1);
    decayBasebandGrid(grid, 0.5);
    expect(grid.cells[0]).toBeCloseTo(BASEBAND_GAIN / 2, 6);
    for (let i = 0; i < 20; i++) {
      decayBasebandGrid(grid, 0.5);
    }
    expect(grid.cells[0]).toBe(0);
  });
});

describe("clearBasebandGrid", () => {
  it("empties every cell", () => {
    const grid = createBasebandGrid(4, 4);
    addConstellation(grid, iq([0, 0]), 1);
    clearBasebandGrid(grid);
    expect(lit(grid)).toEqual([]);
  });
});

describe("samplesPerSymbol", () => {
  it("divides the sample rate by the symbol rate", () => {
    expect(samplesPerSymbol(24_000, 4800)).toBe(5);
  });

  it("falls back to one rather than dividing by zero", () => {
    expect(samplesPerSymbol(24_000, 0)).toBe(1);
    expect(samplesPerSymbol(0, 4800)).toBe(1);
  });
});

function shapedBpsk(symbols: number[], period: number, phase: number): Float32Array {
  const out = new Float32Array(symbols.length * period * 2);
  for (let i = 0; i < symbols.length * period; i++) {
    const since = (i - phase + symbols.length * period) % period;
    const index = Math.floor((i - phase + symbols.length * period) / period) % symbols.length;
    const window = Math.sin((Math.PI * (since + 0.5)) / period);
    out[i * 2] = (symbols[index] ?? 0) * window;
    out[i * 2 + 1] = 0;
  }
  return out;
}

function sampledEnergy(wave: Float32Array, period: number, offset: number): number {
  let total = 0;
  let count = 0;
  for (let i = offset; i * 2 + 1 < wave.length; i += period) {
    total += (wave[i * 2] ?? 0) ** 2 + (wave[i * 2 + 1] ?? 0) ** 2;
    count++;
  }
  return count === 0 ? 0 : total / count;
}

function widestOffset(wave: Float32Array, period: number): number {
  let best = 0;
  for (let offset = 1; offset < period; offset++) {
    if (sampledEnergy(wave, period, offset) > sampledEnergy(wave, period, best)) {
      best = offset;
    }
  }
  return best;
}

describe("symbolPhase", () => {
  it("lands on the instant the eye is widest open", () => {
    const period = 8;
    for (const phase of [0, 2, 5, 7]) {
      const wave = shapedBpsk([1, -1, 1, 1, -1, -1, 1, -1], period, phase);
      const want = widestOffset(wave, period);
      const found = symbolPhase(wave, period);
      const wrapped = Math.min(Math.abs(found - want), period - Math.abs(found - want));
      expect(wrapped).toBeLessThanOrEqual(1);
    }
  });

  it("beats the worst instant it could have picked", () => {
    const period = 8;
    const wave = shapedBpsk([1, -1, 1, 1, -1, -1, 1, -1], period, 2);
    let worst = 0;
    for (let offset = 1; offset < period; offset++) {
      if (sampledEnergy(wave, period, offset) < sampledEnergy(wave, period, worst)) {
        worst = offset;
      }
    }
    expect(sampledEnergy(wave, period, symbolPhase(wave, period))).toBeGreaterThan(
      sampledEnergy(wave, period, worst) * 4,
    );
  });

  it("stays inside the symbol period it was given", () => {
    const wave = shapedBpsk([1, -1, 1, -1], 8, 3);
    const found = symbolPhase(wave, 8);
    expect(found).toBeGreaterThanOrEqual(0);
    expect(found).toBeLessThan(8);
  });

  it("declines to guess when there is not a symbol to look at", () => {
    expect(symbolPhase(new Float32Array(0), 8)).toBe(0);
    expect(symbolPhase(iq([1, 0], [1, 0]), 1)).toBe(0);
  });
});

describe("symbolHistogram", () => {
  it("puts four levels in four separated humps", () => {
    const levels = Float32Array.from(
      Array.from({ length: 400 }, (_, i) => [1, 3, -1, -3][i % 4] ?? 0),
    );
    const bins = symbolHistogram(levels, 1, 3);
    const occupied = [...bins].filter((v) => v > 0).length;
    expect(occupied).toBe(4);
    expect(Math.max(...bins)).toBe(1);
  });

  it("normalises the tallest hump to one whatever the count", () => {
    const few = symbolHistogram(Float32Array.from([1, 1, -1]), 1, 1);
    const many = symbolHistogram(Float32Array.from(Array(300).fill(1)), 1, 1);
    expect(Math.max(...few)).toBe(1);
    expect(Math.max(...many)).toBe(1);
  });

  it("ignores a value that falls outside the rail it was given", () => {
    const bins = symbolHistogram(Float32Array.from([0, 40, -40]), 1, 1);
    expect([...bins].reduce((a, b) => a + b, 0)).toBe(1);
  });

  it("reads only one rail of an interleaved pair when strided", () => {
    const pairs = Float32Array.from([1, -1, 1, -1, 1, -1]);
    const bins = symbolHistogram(pairs, 2, 1);
    const filled = [...bins].map((v, i) => (v > 0 ? i : -1)).filter((i) => i >= 0);
    expect(filled).toHaveLength(1);
  });
});

describe("Trend", () => {
  it("keeps the newest values once it is full", () => {
    const trend = new Trend(3);
    for (const value of [1, 2, 3, 4, 5]) {
      trend.push(value);
    }
    expect(trend.length).toBe(3);
    expect([0, 1, 2].map((i) => trend.sample(i))).toEqual([3, 4, 5]);
  });

  it("refuses a value nothing can plot", () => {
    const trend = new Trend(4);
    trend.push(Number.NaN);
    trend.push(Number.POSITIVE_INFINITY);
    expect(trend.length).toBe(0);
  });

  it("widens a flat range so a line still has somewhere to sit", () => {
    const trend = new Trend(4);
    trend.push(7);
    trend.push(7);
    const { min, max } = trend.range();
    expect(max).toBeGreaterThan(min);
  });

  it("spans the values it holds", () => {
    const trend = new Trend(8);
    for (const value of [-3, 12, 4]) {
      trend.push(value);
    }
    expect(trend.range()).toEqual({ min: -3, max: 12 });
  });

  it("forgets everything on a clear", () => {
    const trend = new Trend(4);
    trend.push(1);
    trend.clear();
    expect(trend.length).toBe(0);
  });
});
