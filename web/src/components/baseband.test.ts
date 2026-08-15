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
