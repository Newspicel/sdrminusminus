import { describe, expect, it } from "vitest";
import type { SymbolFrame } from "../../lib/frame";
import { discriminator, paired, referenceScale, tickLabel, waiting } from "./BasebandView";

function block(over: Partial<SymbolFrame> = {}): SymbolFrame {
  return {
    streamId: 300,
    seq: 0,
    timestamp: 0n,
    plane: "level",
    symbolRate: 4800,
    evm: 0.125,
    merDb: 18.06,
    margin: 2.5,
    freqErrorHz: -12,
    reference: Float32Array.from([1, 3, -1, -3]),
    symbols: Float32Array.from([1, -1, 3, -3]),
    ...over,
  };
}

describe("paired", () => {
  it("leaves a complex cloud as the pairs it already is", () => {
    const cloud = block({ plane: "complex", symbols: Float32Array.from([0.7, 0.7, -0.7, 0.7]) });
    expect(paired(cloud)).toBe(cloud.symbols);
  });

  it("lays a level rail along the real axis so it plots as a line", () => {
    expect(Array.from(paired(block()))).toEqual([1, 0, -1, 0, 3, 0, -3, 0]);
  });
});

describe("referenceScale", () => {
  it("leaves room around the outermost level", () => {
    expect(referenceScale(block())).toBeCloseTo(3 * 1.4);
  });

  it("measures a complex reference by its radius", () => {
    const scale = referenceScale(
      block({ plane: "complex", reference: Float32Array.from([3, 4, -3, -4]) }),
    );
    expect(scale).toBeCloseTo(5 * 1.4);
  });

  it("falls back to a unit rail rather than collapsing to zero", () => {
    expect(referenceScale(block({ reference: new Float32Array(0) }))).toBe(1);
  });
});

describe("waiting", () => {
  it("says a trend needs a decoder that reports symbols", () => {
    expect(waiting("quality", null, null)).toContain("reports no symbols");
    expect(waiting("drift", null, null)).toContain("reports no symbols");
    expect(waiting("states", null, null)).toContain("reports no symbols");
  });

  it("clears once symbols arrive", () => {
    expect(waiting("quality", null, block())).toBeNull();
    expect(waiting("drift", null, block())).toBeNull();
    expect(waiting("states", null, block())).toBeNull();
  });

  it("waits on the first burst for the views baseband can draw", () => {
    expect(waiting("spectrum", null, null)).toContain("No burst yet");
    expect(waiting("constellation", null, null)).toContain("No burst yet");
  });

  it("draws a symbol view from symbols alone when there is no burst yet", () => {
    expect(waiting("levels", null, block())).toBeNull();
  });
});

describe("discriminator", () => {
  it("reads a steady rotation as a steady level", () => {
    const count = 64;
    const wave = new Float32Array(count * 2);
    for (let i = 0; i < count; i++) {
      const phase = (Math.PI / 4) * i;
      wave[i * 2] = Math.cos(phase);
      wave[i * 2 + 1] = Math.sin(phase);
    }
    const rail = discriminator(wave, 4, 1);
    expect(rail.length).toBeGreaterThan(4);
    for (const value of rail) {
      expect(value).toBeCloseTo(0.25, 5);
    }
  });

  it("takes one reading per symbol period", () => {
    const wave = new Float32Array(64 * 2);
    expect(discriminator(wave, 8, 0).length).toBe(8);
    expect(discriminator(wave, 4, 0).length).toBe(16);
  });
});

describe("tickLabel", () => {
  it("rounds to two significant digits", () => {
    expect(tickLabel(0.3536)).toBe("0.35");
    expect(tickLabel(-1234)).toBe("-1200");
    expect(tickLabel(0.5)).toBe("0.5");
  });
});
