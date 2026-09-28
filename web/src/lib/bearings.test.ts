import { afterEach, describe, expect, it } from "vitest";
import { bearingRecord } from "../test/fixtures";
import {
  BEARING_HISTORY,
  BEARING_MAX_AGE_MS,
  bearingOf,
  keepRecent,
  useBearingStore,
} from "./bearings";

afterEach(() => useBearingStore.getState().reset());

describe("bearingOf", () => {
  it("takes a bearing only with a place", () => {
    expect(bearingOf(bearingRecord("df", { lat: null }))).toBeNull();
    expect(bearingOf(bearingRecord("df", { lon: Number.NaN }))).toBeNull();
    expect(bearingOf(bearingRecord("df", {}))).toMatchObject({
      node: "df",
      trueDeg: 137,
      sigmaDeg: 3,
      lat: 52,
      lon: 13,
      at: Date.parse("2026-09-28T12:00:00Z"),
    });
  });

  it("falls back to now when the time does not parse", () => {
    expect(bearingOf(bearingRecord("df", {}, "later"), 42)?.at).toBe(42);
  });

  it("names the node from the bearing when the record carries no origin", () => {
    const record = { ...bearingRecord("df", { node: "finder" }), origin: null };
    expect(bearingOf(record)?.node).toBe("finder");
  });
});

describe("useBearingStore", () => {
  it("keeps 64 bearings per node and drops those older than five minutes", () => {
    const now = Date.now();
    const records = Array.from({ length: 70 }, (_, index) =>
      bearingRecord(
        "df",
        { bearing_deg: index },
        new Date(now - (70 - index) * 1_000).toISOString(),
      ),
    );
    for (const record of records) {
      useBearingStore.getState().observe({ type: "Decoded", data: record });
    }
    const kept = useBearingStore.getState().byNode.df ?? [];
    expect(kept).toHaveLength(BEARING_HISTORY);
    expect(kept.at(-1)?.trueDeg).toBe(69);
    const old = { ...kept[0], at: now - BEARING_MAX_AGE_MS - 1 } as (typeof kept)[number];
    expect(keepRecent([old, ...kept.slice(1)], now)).toHaveLength(BEARING_HISTORY - 1);
  });

  it("prunes bearings older than five minutes as new ones arrive", () => {
    const now = Date.now();
    const stale = new Date(now - BEARING_MAX_AGE_MS - 5_000).toISOString();
    const fresh = new Date(now - 1_000).toISOString();
    useBearingStore.getState().observe({
      type: "DecodedBacklog",
      data: {
        records: [
          bearingRecord("df", { bearing_deg: 1 }, stale),
          bearingRecord("df", { bearing_deg: 2 }, fresh),
          bearingRecord("df", { bearing_deg: 3, lat: null }, fresh),
        ],
      },
    });
    expect(useBearingStore.getState().byNode.df?.map((sample) => sample.trueDeg)).toEqual([2]);
  });

  it("reads backlog records", () => {
    useBearingStore.getState().observe({
      type: "DecodedBacklog",
      data: {
        records: [
          bearingRecord("a", {}, new Date().toISOString()),
          bearingRecord("b", {}, new Date().toISOString()),
        ],
      },
    });
    expect(Object.keys(useBearingStore.getState().byNode).toSorted()).toEqual(["a", "b"]);
  });

  it("forgets a node", () => {
    useBearingStore.getState().observe({
      type: "Decoded",
      data: bearingRecord("a", {}, new Date().toISOString()),
    });
    useBearingStore.getState().forget(["a"]);
    expect(useBearingStore.getState().byNode.a).toBeUndefined();
  });
});
