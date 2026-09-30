import { describe, expect, it } from "vitest";
import type { VisibilityFrame } from "../../lib/frame";
import type { Baseline } from "../../lib/types";
import {
  baselineOptions,
  baselineSeries,
  baselineText,
  channelOptions,
  FRINGE_POINTS,
  linePath,
  phasePath,
  withFringe,
} from "./correlator";

function baseline(a: number, b: number, overrides: Partial<Baseline> = {}): Baseline {
  return { a, b, delay_ns: 1.234, coherence: 0.823, ...overrides };
}

function visibility(): VisibilityFrame {
  return {
    streamId: 1,
    seq: 1,
    timestamp: 0n,
    centerHz: 100e6,
    spanHz: 4e6,
    baselines: 2,
    bins: 4,
    dbMin: -60,
    dbMax: 0,
    amplitude: Uint8Array.from([0, 255, 0, 0, 51, 102, 153, 255]),
    phase: Uint8Array.from([0, 0, 0, 0, 0, 128, 255, 64]),
  };
}

describe("correlator baselines", () => {
  it("offers one choice per baseline, numbered from one", () => {
    expect(baselineOptions([baseline(0, 1), baseline(0, 2), baseline(1, 2)])).toEqual([
      { value: 0, label: "1-2" },
      { value: 1, label: "1-3" },
      { value: 2, label: "2-3" },
    ]);
  });

  it("sums a baseline up in one row", () => {
    expect(baselineText(baseline(0, 1, { phase_deg: 37.2, snr_db: 18.4 }))).toBe(
      "0.82 ∠37° 1.2 ns 18 dB",
    );
  });

  it("offers no more channels than the FFT has bins", () => {
    const options = channelOptions(64);
    expect(options.find((option) => option.value === 64)?.disabled).toBe(false);
    expect(options.find((option) => option.value === 128)?.disabled).toBe(true);
  });
});

describe("baselineSeries", () => {
  it("decodes one baseline's bytes into dB and degrees", () => {
    const series = baselineSeries(visibility(), 1);
    expect(series.hz).toEqual([98.5e6, 99.5e6, 100.5e6, 101.5e6]);
    expect(series.db.map((db) => Math.round(db))).toEqual([-48, -36, -24, 0]);
    expect(series.deg[0]).toBe(-180);
    expect(series.deg[2]).toBe(180);
    expect(series.deg[1]).toBeCloseTo(0.7, 1);
  });

  it("returns nothing for a baseline the frame lacks", () => {
    expect(baselineSeries(visibility(), 5).hz).toEqual([]);
  });
});

describe("plot paths", () => {
  it("breaks the phase path on wraps", () => {
    const path = phasePath(
      [0, 1, 2, 3],
      [170, 179, -179, -170],
      { w: 30, h: 360 },
      {
        lo: 0,
        hi: 3,
      },
    );
    expect(path.match(/M/g)).toHaveLength(2);
    expect(path.startsWith("M0.0 10.0L10.0 1.0M20.0 359.0")).toBe(true);
  });

  it("draws a line through every point", () => {
    expect(linePath([0, 1], [0, 10], { w: 10, h: 10 }, { lo: 0, hi: 1 }, { lo: 0, hi: 10 })).toBe(
      "M0.0 10.0L10.0 0.0",
    );
  });

  it("keeps a bounded fringe history", () => {
    let history: number[] = [];
    for (let step = 0; step < FRINGE_POINTS + 5; step++) {
      history = withFringe(history, step);
    }
    expect(history).toHaveLength(FRINGE_POINTS);
    expect(history[0]).toBe(5);
  });
});
