import { describe, expect, it } from "vitest";
import { clickRateHz, FASTEST_HZ, SLOWEST_HZ } from "./geiger";

describe("clickRateHz", () => {
  it("spans the whole rate range across the whole strength range", () => {
    expect(clickRateHz(0)).toBe(SLOWEST_HZ);
    expect(clickRateHz(1)).toBe(FASTEST_HZ);
  });

  it("climbs, so closing on a transmitter always sounds faster", () => {
    const steps = [0, 0.2, 0.4, 0.6, 0.8, 1].map(clickRateHz);
    for (let i = 1; i < steps.length; i += 1) {
      expect(steps[i]).toBeGreaterThan(steps[i - 1] as number);
    }
  });

  it("spends its resolution near the transmitter, where the last steps matter", () => {
    expect(clickRateHz(0.9) - clickRateHz(0.8)).toBeGreaterThan(
      clickRateHz(0.2) - clickRateHz(0.1),
    );
  });

  it("refuses to be driven out of range by a reading that makes no sense", () => {
    for (const bad of [Number.NaN, Number.POSITIVE_INFINITY, -5, 12]) {
      const rate = clickRateHz(bad);
      expect(rate).toBeGreaterThanOrEqual(SLOWEST_HZ);
      expect(rate).toBeLessThanOrEqual(FASTEST_HZ);
    }
  });
});
