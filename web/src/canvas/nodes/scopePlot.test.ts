import { describe, expect, it } from "vitest";
import { FULL_VIEW } from "../../components/spectrumView";
import { type PlotFrame, readoutAt, tracePoints } from "./scopePlot";

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

describe("tracePoints", () => {
  it("keeps the peak and the floor of the bins under each pixel", () => {
    const points = tracePoints(Float32Array.from([-90, -40, -80, -85]), FULL_VIEW, 1.5);
    expect(points.count).toBe(2);
    expect(points.high[0]).toBe(-40);
    expect(points.low[0]).toBe(-90);
    expect(points.high[1]).toBe(-80);
    expect(points.low[1]).toBe(-85);
  });

  it("draws through bin centres when zoomed past one bin per pixel", () => {
    const points = tracePoints(Float32Array.from([-90, -60, -80]), FULL_VIEW, 100);
    expect(points.count).toBe(3);
    expect([...points.xs.subarray(0, 3)]).toEqual([0, 50, 100]);
    expect([...points.high.subarray(0, 3)]).toEqual([-90, -60, -80]);
  });

  it("places the bins of a zoomed window on its own scale", () => {
    const points = tracePoints(
      Float32Array.from([-90, -60, -80, -70, -50]),
      { start: 0.5, end: 1 },
      100,
    );
    expect(points.xs[0]).toBe(0);
    expect(points.high[0]).toBe(-80);
    expect(points.xs[points.count - 1]).toBe(100);
  });

  it("has nothing to draw without bins", () => {
    expect(tracePoints(Float32Array.of(-90), FULL_VIEW, 100).count).toBe(0);
  });
});
