import { describe, expect, it } from "vitest";
import type { SymbolFrame } from "../../lib/frame";
import {
  discriminator,
  formatMeasurement,
  paired,
  readout,
  referenceScale,
  waiting,
} from "./BasebandView";

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

describe("formatMeasurement", () => {
  it("states the rate, the error, the margin and the offset", () => {
    const text = formatMeasurement(block());
    expect(text).toContain("4.80 kBd");
    expect(text).toContain("12.5% EVM");
    expect(text).toContain("18.1 dB MER");
    expect(text).toContain("×2.50 margin");
    expect(text).toContain("-12 Hz");
  });

  it("says clean rather than printing the ceiling as a measurement", () => {
    expect(formatMeasurement(block({ merDb: 99 }))).toContain("clean");
  });

  it("keeps a slow mode in baud", () => {
    expect(formatMeasurement(block({ symbolRate: 31.25 }))).toContain("31.25 Bd");
  });
});

describe("waiting", () => {
  it("says a trend needs a decoder that reports symbols", () => {
    expect(waiting("quality", null, null)).toContain("does not report symbols");
    expect(waiting("drift", null, null)).toContain("does not report symbols");
  });

  it("clears once symbols arrive", () => {
    expect(waiting("quality", null, block())).toBeNull();
    expect(waiting("drift", null, block())).toBeNull();
  });

  it("waits on the first burst for the views baseband can draw", () => {
    expect(waiting("spectrum", null, null)).toContain("first burst");
    expect(waiting("constellation", null, null)).toContain("first burst");
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

describe("readout", () => {
  const burst = {
    streamId: 1,
    seq: 0,
    timestamp: 0n,
    sampleRate: 48_000,
    centerHz: 145.8e6,
    samples: Float32Array.from([1, 0]),
  };

  it("keeps the spectrum readout on a channel that also reports symbols", () => {
    const text = readout("spectrum", burst, block(), 10);
    expect(text).toContain("145.8000 MHz");
    expect(text).not.toContain("EVM");
  });

  it("keeps the eye on its own sample-rate readout", () => {
    expect(readout("eye", burst, block(), 10)).toContain("Sa/sym");
  });

  it("shows the measurement on the views the symbols feed", () => {
    for (const view of ["constellation", "levels", "quality", "drift"] as const) {
      expect(readout(view, burst, block(), 10)).toContain("EVM");
    }
  });

  it("falls back to the burst readout when no decoder reports symbols", () => {
    expect(readout("constellation", burst, null, 10)).toContain("145.8000 MHz");
  });

  it("says nothing at all before anything has arrived", () => {
    expect(readout("spectrum", null, null, 0)).toBe("");
  });
});
