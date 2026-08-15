import { describe, expect, it } from "vitest";
import type { RowMeta, SpectrumHistory } from "../lib/spectrum";
import { frozenAge, frozenCursor, frozenLength, frozenRow } from "./spectrumFreeze";

function history(rows: number[][], meta: Partial<RowMeta>[]): SpectrumHistory {
  const bins = rows[0]?.length ?? 0;
  return {
    rows: Uint8Array.from(rows.flat()),
    count: rows.length,
    bins,
    meta: meta.map((entry, i) => ({
      centerHz: 100e6,
      spanHz: 2e6,
      dbMin: -100,
      dbMax: -20,
      at: 1000 + i * 100,
      ...entry,
    })),
  };
}

describe("frozenRow", () => {
  it("reads a row back through its own dB window", () => {
    const frozen = history(
      [
        [0, 255],
        [0, 255],
      ],
      [
        { dbMin: -100, dbMax: -20 },
        { dbMin: -60, dbMax: 0 },
      ],
    );

    expect(frozenRow(frozen, 0)?.db[0]).toBe(-100);
    expect(frozenRow(frozen, 0)?.db[1]).toBe(-20);
    // The second row was measured under a window that had already moved; reading it under the
    // first one's would put the same bytes 40 dB away from where they were measured.
    expect(frozenRow(frozen, 1)?.db[0]).toBe(-60);
    expect(frozenRow(frozen, 1)?.db[1]).toBe(0);
  });

  it("carries the row's own centre and span", () => {
    const frozen = history([[1]], [{ centerHz: 433.92e6, spanHz: 1e6 }]);
    const row = frozenRow(frozen, 0);
    expect(row).toMatchObject({ centerHz: 433.92e6, spanHz: 1e6, at: 1000 });
    expect(row?.window).toEqual({ min: -100, max: -20 });
  });

  it("clamps an index that runs off either end", () => {
    const frozen = history([[0], [255]], [{}, {}]);
    expect(frozenRow(frozen, -5)?.db[0]).toBe(-100);
    expect(frozenRow(frozen, 99)?.db[0]).toBe(-20);
  });

  it("reuses the output buffer while the bin count is unchanged", () => {
    const frozen = history([[1, 2]], [{}]);
    const out = new Float32Array(2);
    expect(frozenRow(frozen, 0, out)?.db).toBe(out);
  });

  it("has nothing to read from an empty history", () => {
    expect(frozenRow({ rows: new Uint8Array(0), count: 0, bins: 0, meta: [] }, 0)).toBeNull();
  });
});

describe("frozenLength", () => {
  it("is the shorter of the rows and their metadata", () => {
    expect(frozenLength(history([[1], [2], [3]], [{}, {}, {}]))).toBe(3);
    expect(frozenLength({ ...history([[1], [2]], [{}]), count: 2 })).toBe(1);
  });
});

describe("frozenCursor", () => {
  it("puts the newest row at the top of the plot", () => {
    expect(frozenCursor(9, 10, 100)).toBeCloseTo(0.005, 6);
  });

  it("moves down the plot as the scrub reaches back", () => {
    const near = frozenCursor(8, 10, 100) ?? 0;
    const far = frozenCursor(5, 10, 100) ?? 0;
    expect(far).toBeGreaterThan(near);
  });

  it("reports nothing for a row scrolled off the bottom", () => {
    expect(frozenCursor(0, 500, 100)).toBeNull();
  });

  it("reports nothing when there is no history or no plot", () => {
    expect(frozenCursor(0, 0, 100)).toBeNull();
    expect(frozenCursor(0, 10, 0)).toBeNull();
  });
});

describe("frozenAge", () => {
  it("counts back from the newest row kept", () => {
    const frozen = history([[1], [2], [3]], [{ at: 1000 }, { at: 1500 }, { at: 2000 }]);
    expect(frozenAge(frozen, 2)).toBe("live edge");
    expect(frozenAge(frozen, 1)).toBe("−0.5 s");
    expect(frozenAge(frozen, 0)).toBe("−1.0 s");
  });

  it("says nothing about an empty history", () => {
    expect(frozenAge({ rows: new Uint8Array(0), count: 0, bins: 0, meta: [] }, 0)).toBe("");
  });
});
