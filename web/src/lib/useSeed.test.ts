import { describe, expect, it } from "vitest";
import { plantSeed, type Seed, seedAlert, settledSeed } from "./useSeed";

describe("settledSeed", () => {
  it("holds data back while a refetch is running", () => {
    const old = { samples: 1 };
    expect(settledSeed({ data: old, dataUpdatedAt: 5, isFetching: true })).toEqual({
      data: undefined,
      at: 5,
    });
    expect(settledSeed({ data: old, dataUpdatedAt: 9, isFetching: false })).toEqual({
      data: old,
      at: 9,
    });
  });

  it("plants the refetched data, not the cached copy, after a reconnect", () => {
    const planted: { current: Seed<{ samples: number }> | null } = { current: null };
    const applied: number[] = [];
    const plant = (seed: Seed<{ samples: number }>) =>
      plantSeed(planted, seed, applied.length > 0, (data) => applied.push(data.samples));
    plant(settledSeed({ data: { samples: 1 }, dataUpdatedAt: 1, isFetching: true }));
    plant(settledSeed({ data: { samples: 7 }, dataUpdatedAt: 2, isFetching: false }));
    expect(applied).toEqual([7]);
  });
});

describe("seedAlert", () => {
  it("flags a failed seed only while no live state is held", () => {
    const failed = new Error("no triangulation tri");
    expect(seedAlert(failed, false)).toBe("flag");
    expect(seedAlert(failed, true)).toBe("clear");
    expect(seedAlert(null, true)).toBe("clear");
    expect(seedAlert(null, false)).toBeNull();
  });
});

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
