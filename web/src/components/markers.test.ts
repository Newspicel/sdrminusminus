import { describe, expect, it } from "vitest";
import { markerFraction } from "./markers";

describe("markerFraction", () => {
  it("maps center to the middle and the span edges to 0/1", () => {
    expect(markerFraction(0, 2_000_000)).toBe(0.5);
    expect(markerFraction(-1_000_000, 2_000_000)).toBe(0);
    expect(markerFraction(1_000_000, 2_000_000)).toBe(1);
  });

  it("scales linearly inside the span", () => {
    expect(markerFraction(500_000, 2_000_000)).toBe(0.75);
    expect(markerFraction(-250_000, 2_000_000)).toBe(0.375);
  });

  it("hides out-of-view and degenerate cases", () => {
    expect(markerFraction(1_100_000, 2_000_000)).toBeNull();
    expect(markerFraction(-1_000_001, 2_000_000)).toBeNull();
    expect(markerFraction(0, 0)).toBeNull();
    expect(markerFraction(0, -1)).toBeNull();
    expect(markerFraction(Number.NaN, 2_000_000)).toBeNull();
  });
});
