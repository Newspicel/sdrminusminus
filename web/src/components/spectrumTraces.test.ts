import { describe, expect, it } from "vitest";
import type { SpectrumFrame } from "../lib/frame";
import {
  accumulateTraces,
  binDb,
  dequantize,
  frameWindow,
  newTraceState,
  requantize,
  requantizeHistory,
  traceOf,
  traceUnit,
} from "./spectrumTraces";

function frame(bins: number[], dbMin: number, dbMax: number): SpectrumFrame {
  return {
    streamId: 0,
    seq: 0,
    timestamp: 0n,
    centerHz: 100e6,
    spanHz: 2e6,
    dbMin,
    dbMax,
    bins: Uint8Array.from(bins),
  };
}

describe("dequantize", () => {
  it("maps the byte range onto the frame's dB window", () => {
    const db = dequantize(frame([0, 128, 255], -100, -20), null);
    expect(db[0]).toBe(-100);
    expect(db[1]).toBeCloseTo(-59.8, 1);
    expect(db[2]).toBe(-20);
  });

  it("reuses the output while the bin count is unchanged", () => {
    const out = new Float32Array(3);
    expect(dequantize(frame([1, 2, 3], -100, -20), out)).toBe(out);
    expect(dequantize(frame([1, 2], -100, -20), out)).not.toBe(out);
  });
});

describe("requantize", () => {
  it("keeps a level in place when the window moves under it", () => {
    // Byte 159 under [−100, −20] is −50 dB. Under [−100, 0] that same level is byte 127.
    const moved = requantize(
      Uint8Array.of(159),
      { min: -100, max: -20 },
      { min: -100, max: 0 },
      null,
    );
    expect(moved[0]).toBe(127);
  });

  it("is the identity when the windows match", () => {
    const window = { min: -100, max: -20 };
    const same = requantize(Uint8Array.of(0, 64, 200, 255), window, window, null);
    expect(Array.from(same)).toEqual([0, 64, 200, 255]);
  });

  it("clamps levels outside the target window", () => {
    const out = requantize(
      Uint8Array.of(0, 255),
      { min: -140, max: 0 },
      { min: -100, max: -20 },
      null,
    );
    expect(Array.from(out)).toEqual([0, 255]);
  });

  it("floors everything onto an empty window rather than dividing by zero", () => {
    const out = requantize(
      Uint8Array.of(0, 128, 255),
      { min: -100, max: -20 },
      { min: -50, max: -50 },
      null,
    );
    expect(Array.from(out)).toEqual([0, 0, 0]);
  });
});

describe("requantizeHistory", () => {
  it("brings rows measured under different windows onto one scale", () => {
    const history = {
      rows: Uint8Array.of(159, 0, 127, 0),
      count: 2,
      bins: 2,
      // −50 dB reads as byte 159 under the first window and as 127 under the second.
      meta: [
        { dbMin: -100, dbMax: -20 },
        { dbMin: -100, dbMax: 0 },
      ],
    };

    const rows = requantizeHistory(history, { min: -100, max: 0 });
    expect(rows[0]).toBe(127);
    expect(rows[2]).toBe(127);
  });

  it("passes a row through untouched when its window was never recorded", () => {
    const history = { rows: Uint8Array.of(7, 9), count: 1, bins: 2, meta: [] };
    expect(Array.from(requantizeHistory(history, { min: -100, max: 0 }))).toEqual([7, 9]);
  });
});

describe("accumulateTraces", () => {
  it("tracks the loudest, quietest and mean level of each bin", () => {
    let state = accumulateTraces(null, Float32Array.of(-60, -30));
    state = accumulateTraces(state, Float32Array.of(-40, -50));

    expect(Array.from(traceOf(state, "peak"))).toEqual([-40, -30]);
    expect(Array.from(traceOf(state, "min"))).toEqual([-60, -50]);
    expect(Array.from(traceOf(state, "average"))).toEqual([-50, -40]);
  });

  it("takes the first frame as the average outright", () => {
    const state = accumulateTraces(null, Float32Array.of(-77));
    expect(state.average[0]).toBe(-77);
    expect(state.frames).toBe(1);
  });

  it("converges on a steady level and does not overshoot it", () => {
    let state: ReturnType<typeof newTraceState> | null = null;
    for (let i = 0; i < 200; i++) {
      state = accumulateTraces(state, Float32Array.of(i === 0 ? -20 : -80));
    }
    expect(state?.average[0]).toBeCloseTo(-80, 1);
    // The peak remembers the burst the average has forgotten — which is the point of having both.
    expect(state?.peak[0]).toBe(-20);
  });

  it("reuses the state until the bin count changes", () => {
    const state = accumulateTraces(null, Float32Array.of(-60, -60));
    expect(accumulateTraces(state, Float32Array.of(-50, -50))).toBe(state);
    const wider = accumulateTraces(state, Float32Array.of(-50, -50, -50));
    expect(wider).not.toBe(state);
    expect(wider.frames).toBe(1);
  });
});

describe("traceUnit", () => {
  it("places a level on the plot's unit height", () => {
    expect(traceUnit(-60, { min: -100, max: -20 })).toBeCloseTo(0.5, 6);
    expect(traceUnit(-100, { min: -100, max: -20 })).toBe(0);
    expect(traceUnit(-20, { min: -100, max: -20 })).toBe(1);
  });

  it("clamps outside the window and floors an absent level", () => {
    expect(traceUnit(-140, { min: -100, max: -20 })).toBe(0);
    expect(traceUnit(10, { min: -100, max: -20 })).toBe(1);
    expect(traceUnit(Number.NEGATIVE_INFINITY, { min: -100, max: -20 })).toBe(0);
    expect(traceUnit(-60, { min: -20, max: -20 })).toBe(0);
  });
});

describe("binDb and frameWindow", () => {
  it("read a byte back through its own frame's window", () => {
    const source = frame([0, 255], -110, -10);
    const window = frameWindow(source);
    expect(window).toEqual({ min: -110, max: -10 });
    expect(binDb(0, window)).toBe(-110);
    expect(binDb(255, window)).toBe(-10);
  });
});
