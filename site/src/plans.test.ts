import { describe, expect, it } from "vitest";
import { euros, lowest, monthsFree, PLANS, type Plan, people, sites } from "./plans";

const plan = (monthly: number, yearly: number): Plan => ({
  name: "Test",
  monthly,
  yearly,
  perSeat: false,
  minSeats: 1,
  sitesPerSeat: 3,
});

const team: Plan = { ...plan(4, 40), perSeat: true, minSeats: 3, sitesPerSeat: 10 };

describe("monthsFree", () => {
  it("counts whole months a year saves", () => {
    expect(monthsFree(plan(3, 30))).toBe(2);
    expect(monthsFree(plan(4, 40))).toBe(2);
  });

  it("is zero when a year costs twelve months", () => {
    expect(monthsFree(plan(3, 36))).toBe(0);
  });
});

describe("people and sites", () => {
  it("are fixed for a single seat plan", () => {
    expect(people(plan(3, 30))).toBe("1");
    expect(sites(plan(3, 30))).toBe("3");
  });

  it("scale with seats", () => {
    expect(people(team)).toBe("3 or more");
    expect(sites(team)).toBe("10 per seat");
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
