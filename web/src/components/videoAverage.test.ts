import { describe, expect, it } from "vitest";
import { quantizeDb, VideoAverage } from "./videoAverage";

describe("VideoAverage", () => {
  it("passes frames through when off", () => {
    const db = Float32Array.of(-80);
    expect(new VideoAverage().apply(db, 1)).toBe(db);
  });

  it("starts from the first frame and moves a share of the way per frame", () => {
    const average = new VideoAverage();
    expect(average.apply(Float32Array.of(-80), 4)[0]).toBeCloseTo(-80);
    const next = average.apply(Float32Array.of(-70), 4)[0] ?? 0;
    const expected = 10 * Math.log10(10 ** -8 + (10 ** -7 - 10 ** -8) / 4);
    expect(next).toBeCloseTo(expected, 4);
  });

  it("holds a steady level", () => {
    const average = new VideoAverage();
    for (let i = 0; i < 20; i++) {
      average.apply(Float32Array.of(-50, -90), 8);
    }
    const out = average.apply(Float32Array.of(-50, -90), 8);
    expect(out[0]).toBeCloseTo(-50, 3);
    expect(out[1]).toBeCloseTo(-90, 3);
  });

  it("forgets the past on reset", () => {
    const average = new VideoAverage();
    average.apply(Float32Array.of(-80), 8);
    average.reset();
    expect(average.apply(Float32Array.of(-20), 8)[0]).toBeCloseTo(-20);
  });
});

describe("quantizeDb", () => {
  it("spreads the window over the byte range", () => {
    expect([
      ...quantizeDb(Float32Array.of(-130, -120, 8, 135, 140), { min: -120, max: 135 }, null),
    ]).toEqual([0, 0, 128, 255, 255]);
  });
});
