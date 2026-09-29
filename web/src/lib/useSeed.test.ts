import { describe, expect, it } from "vitest";
import { plantSeed, type Seed } from "./useSeed";

interface Seen {
  samples: number;
}

function planter() {
  const planted: { current: Seed<Seen> | null } = { current: null };
  const applied: Seen[] = [];
  const plant = (data: Seen | undefined, at: number, held: boolean) =>
    plantSeed(planted, { data, at }, held, (seed) => applied.push(seed));
  return { plant, applied };
}

describe("plantSeed", () => {
  it("applies a seed once and never over live state", () => {
    const { plant, applied } = planter();
    const first = { samples: 1 };
    expect(plant(undefined, 0, false)).toBe(false);
    expect(plant(first, 1, false)).toBe(true);
    expect(plant(first, 1, false)).toBe(false);
    const late = { samples: 2 };
    expect(plant(late, 2, true)).toBe(true);
    expect(plant(late, 2, false)).toBe(false);
    const refetched = { samples: 3 };
    expect(plant(refetched, 3, false)).toBe(true);
    expect(applied).toEqual([first, refetched]);
  });

  it("applies an unchanged refetch after the live state was cleared", () => {
    const { plant, applied } = planter();
    const kept = { samples: 4 };
    plant(kept, 1, false);
    expect(plant(kept, 1, false)).toBe(false);
    expect(plant(kept, 2, false)).toBe(true);
    expect(applied).toEqual([kept, kept]);
  });
});
