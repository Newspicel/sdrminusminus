import { describe, expect, it } from "vitest";
import { niceStep, niceTicks, stepDecimals, tickLabel } from "./ticks";

describe("niceTicks", () => {
  it("picks 1, 2 or 5 steps", () => {
    expect(niceTicks(0, 32, 5)).toEqual([0, 10, 20, 30]);
    expect(niceTicks(-100, 100, 4)).toEqual([-100, -50, 0, 50, 100]);
    expect(niceStep(10, 5)).toBe(2);
    expect(niceStep(1, 4)).toBe(0.5);
  });

  it("keeps decimal ticks clean", () => {
    expect(niceTicks(0, 1, 5)).toEqual([0, 0.2, 0.4, 0.6, 0.8, 1]);
  });

  it("copes with an empty or reversed span", () => {
    expect(niceTicks(5, 5, 4)).toEqual([5]);
    expect(niceTicks(10, 0, 2)).toEqual([0, 5, 10]);
  });
});

describe("tickLabel", () => {
  it("labels with the decimals the step needs", () => {
    expect(stepDecimals(10)).toBe(0);
    expect(stepDecimals(0.5)).toBe(1);
    expect(stepDecimals(0.05)).toBe(2);
    expect(tickLabel(0.4, 0.2)).toBe("0.4");
    expect(tickLabel(150, 50)).toBe("150");
    expect(tickLabel(0.05, 0.05)).toBe("0.05");
  });
});
