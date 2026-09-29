import { describe, expect, it } from "vitest";
import { euros, lowest, monthsFree, PLANS, type Plan } from "./plans";

const plan = (monthly: number, yearly: number): Plan => ({
  name: "Test",
  monthly,
  yearly,
  perSeat: false,
  minSeats: 1,
});

describe("monthsFree", () => {
  it("counts whole months a year saves", () => {
    expect(monthsFree(plan(3, 30))).toBe(2);
    expect(monthsFree(plan(4, 40))).toBe(2);
  });

  it("is zero when a year costs twelve months", () => {
    expect(monthsFree(plan(3, 36))).toBe(0);
  });
});

describe("plans", () => {
  it("make a year cheaper than twelve months", () => {
    for (const candidate of PLANS) {
      expect(candidate.yearly).toBeLessThan(candidate.monthly * 12);
    }
  });

  it("start at the cheapest monthly price", () => {
    expect(euros(lowest(PLANS))).toBe("€3");
  });
});
