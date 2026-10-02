import { describe, expect, it } from "vitest";
import { gridStep } from "./grid";

describe("gridStep", () => {
  it("coarsens when zoomed out and refines when zoomed in", () => {
    expect(gridStep(0.15)).toBe(48);
    expect(gridStep(0.49)).toBe(48);
    expect(gridStep(0.5)).toBe(24);
    expect(gridStep(1)).toBe(24);
    expect(gridStep(1.25)).toBe(12);
    expect(gridStep(2)).toBe(12);
  });

  it("nests every step inside the coarser ones", () => {
    const [coarse, base, fine] = [gridStep(0.15), gridStep(1), gridStep(2)];
    expect(coarse % base).toBe(0);
    expect(base % fine).toBe(0);
  });
});
