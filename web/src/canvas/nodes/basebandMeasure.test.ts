import { describe, expect, it } from "vitest";
import type { IqFrame, SymbolFrame } from "../../lib/frame";
import { measurements } from "./basebandMeasure";

const burst: IqFrame = {
  streamId: 1,
  seq: 0,
  timestamp: 0n,
  sampleRate: 48_000,
  centerHz: 145.8e6,
  samples: Float32Array.from([1, 0]),
};

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

const labels = (rows: { label: string }[]) => rows.map((row) => row.label);
const value = (rows: { label: string; value: string }[], label: string) =>
  rows.find((row) => row.label === label)?.value;

describe("measurements", () => {
  it("keeps the spectrum to the burst it was drawn from", () => {
    expect(labels(measurements("spectrum", burst, block(), 10))).toEqual(["Centre", "Rate"]);
  });

  it("adds the fold for the eye but not the decoder's figures", () => {
    const rows = measurements("eye", burst, block(), 10);
    expect(labels(rows)).toEqual(["Centre", "Rate", "Sam/sym"]);
    expect(value(rows, "Sam/sym")).toBe("10.00");
  });

  it("reads the decoder's figures on the views the symbols feed", () => {
    for (const view of ["constellation", "levels", "states", "quality", "drift"]) {
      const rows = measurements(view, burst, block(), 10);
      expect(value(rows, "EVM")).toBe("12.5 %");
      expect(value(rows, "MER")).toBe("18.1 dB");
      expect(value(rows, "Offset")).toBe("-12 Hz");
    }
  });

  it("calls a perfect burst clean and signs a positive offset", () => {
    const rows = measurements("levels", burst, block({ merDb: 99, freqErrorHz: 7.4 }), 10);
    expect(value(rows, "MER")).toBe("clean");
    expect(value(rows, "Offset")).toBe("+7 Hz");
  });

  it("folds at the set rate when no decoder reports symbols", () => {
    expect(labels(measurements("constellation", burst, null, 10))).toContain("Sam/sym");
  });

  it("has nothing before anything arrives", () => {
    expect(measurements("spectrum", null, null, 0)).toEqual([]);
  });
});
