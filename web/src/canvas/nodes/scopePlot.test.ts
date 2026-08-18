import { describe, expect, it } from "vitest";
import { FULL_VIEW } from "../../components/spectrumView";
import { type PlotFrame, readoutAt } from "./scopePlot";

function frame(db: number[]): PlotFrame {
  return { centerHz: 100_000_000, spanHz: 2_000_000, db: Float32Array.from(db) };
}

describe("readoutAt", () => {
  it("reads the frequency and level under the cursor", () => {
    const read = readoutAt(frame([-90, -80, -70, -60, -50]), FULL_VIEW, 0.5);
    expect(read?.hz).toBe(100_000_000);
    expect(read?.db).toBe(-70);
  });

  it("reads the edges of the span", () => {
    expect(readoutAt(frame([-90, -80, -70]), FULL_VIEW, 0)?.hz).toBe(99_000_000);
    expect(readoutAt(frame([-90, -80, -70]), FULL_VIEW, 1)?.hz).toBe(101_000_000);
  });

  it("follows a zoomed window", () => {
    const read = readoutAt(frame([-90, -80, -70, -60, -50]), { start: 0.5, end: 1 }, 0.5);
    expect(read?.hz).toBe(100_500_000);
    expect(read?.db).toBe(-60);
  });

  it("gives nothing outside the plot or without a span", () => {
    expect(readoutAt(frame([-90, -80]), FULL_VIEW, -0.1)).toBeNull();
    expect(readoutAt(frame([-90, -80]), FULL_VIEW, 1.1)).toBeNull();
    expect(readoutAt({ ...frame([-90, -80]), spanHz: 0 }, FULL_VIEW, 0.5)).toBeNull();
  });
});
