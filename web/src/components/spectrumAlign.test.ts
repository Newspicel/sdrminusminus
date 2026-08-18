import { describe, expect, it } from "vitest";
import type { RowMeta, SpectrumHistory } from "../lib/spectrum";
import { alignHistory, binShift, retuneAction } from "./spectrumAlign";

function history(rows: number[][], meta: Partial<RowMeta>[]): SpectrumHistory {
  const bins = rows[0]?.length ?? 0;
  const flat = new Uint8Array(rows.length * bins);
  rows.forEach((row, i) => flat.set(row, i * bins));
  return {
    rows: flat,
    count: rows.length,
    bins,
    meta: meta.map((m) => ({
      centerHz: 100e6,
      spanHz: 2e6,
      dbMin: -100,
      dbMax: -20,
      at: 0,
      ...m,
    })),
  };
}

describe("retuneAction", () => {
  it("does nothing before the first frame and while the tuning holds still", () => {
    const key = { centerHz: 100e6, spanHz: 2e6, bins: 1024 };
    expect(retuneAction(null, key)).toEqual({ kind: "none" });
    expect(retuneAction(key, { ...key })).toEqual({ kind: "none" });
  });

  it("shifts by the centre move as a fraction of the span", () => {
    const prev = { centerHz: 100e6, spanHz: 2e6, bins: 1024 };
    expect(retuneAction(prev, { ...prev, centerHz: 100.5e6 })).toEqual({
      kind: "shift",
      delta: 0.25,
    });
    expect(retuneAction(prev, { ...prev, centerHz: 99.5e6 })).toEqual({
      kind: "shift",
      delta: -0.25,
    });
  });

  it("reseeds when the span or the resolution changes", () => {
    const prev = { centerHz: 100e6, spanHz: 2e6, bins: 1024 };
    expect(retuneAction(prev, { ...prev, spanHz: 1e6 })).toEqual({ kind: "reseed" });
    expect(retuneAction(prev, { ...prev, bins: 2048 })).toEqual({ kind: "reseed" });
    expect(retuneAction(prev, { centerHz: 101e6, spanHz: 0, bins: 1024 })).toEqual({
      kind: "reseed",
    });
  });
});

describe("binShift", () => {
  it("moves a row by whole bins toward the new centre", () => {
    const row = { centerHz: 100e6, spanHz: 4e6 };
    expect(binShift(row, { centerHz: 101e6, spanHz: 4e6 }, 4)).toBe(1);
    expect(binShift(row, { centerHz: 99e6, spanHz: 4e6 }, 4)).toBe(-1);
    expect(binShift(row, row, 4)).toBe(0);
  });

  it("gives up on a span change or a move past the whole row", () => {
    const row = { centerHz: 100e6, spanHz: 4e6 };
    expect(binShift(row, { centerHz: 100e6, spanHz: 2e6 }, 4)).toBeNull();
    expect(binShift(row, { centerHz: 104e6, spanHz: 4e6 }, 4)).toBeNull();
    expect(binShift(row, { centerHz: 100e6, spanHz: 0 }, 4)).toBeNull();
  });
});

describe("alignHistory", () => {
  it("slides an old row so its signal stays at its absolute frequency", () => {
    const past = history(
      [
        [1, 2, 3, 4],
        [5, 6, 7, 8],
      ],
      [{ centerHz: 100e6 }, { centerHz: 100.5e6 }],
    );
    const aligned = alignHistory(past, { centerHz: 100.5e6, spanHz: 2e6 }, null);
    expect([...aligned.subarray(0, 4)]).toEqual([2, 3, 4, 0]);
    expect([...aligned.subarray(4, 8)]).toEqual([5, 6, 7, 8]);
  });

  it("pads on the left when the centre moved down", () => {
    const past = history([[1, 2, 3, 4]], [{ centerHz: 100e6 }]);
    const aligned = alignHistory(past, { centerHz: 99.5e6, spanHz: 2e6 }, null);
    expect([...aligned]).toEqual([0, 1, 2, 3]);
  });

  it("blanks a row recorded at another span", () => {
    const past = history(
      [
        [1, 2, 3, 4],
        [5, 6, 7, 8],
      ],
      [{ spanHz: 1e6 }, {}],
    );
    const aligned = alignHistory(past, { centerHz: 100e6, spanHz: 2e6 }, null);
    expect([...aligned.subarray(0, 4)]).toEqual([0, 0, 0, 0]);
    expect([...aligned.subarray(4, 8)]).toEqual([5, 6, 7, 8]);
  });

  it("requantizes into a held window while it slides", () => {
    const past = history([[255, 255, 255, 255]], [{ centerHz: 100.5e6, dbMin: -100, dbMax: -20 }]);
    const aligned = alignHistory(past, { centerHz: 100e6, spanHz: 2e6 }, { min: -100, max: 0 });
    expect([...aligned]).toEqual([0, 204, 204, 204]);
  });

  it("keeps a row without metadata where it was", () => {
    const past = { ...history([[9, 9, 9, 9]], []), meta: [] };
    const aligned = alignHistory(past, { centerHz: 100e6, spanHz: 2e6 }, null);
    expect([...aligned]).toEqual([9, 9, 9, 9]);
  });
});
