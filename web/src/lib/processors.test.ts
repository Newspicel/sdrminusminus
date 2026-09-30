import { afterEach, describe, expect, it } from "vitest";
import { detection, radarUpdate } from "../test/fixtures";
import { isStale, readingOf, useProcessorStore } from "./processors";
import type { ServerEvent } from "./types";

function radar(node: string, detections: ReturnType<typeof detection>[]): ServerEvent {
  return {
    type: "ProcessorUpdate",
    data: { node, reading: { type: "passive_radar", reading: radarUpdate(detections) } },
  };
}

afterEach(() => useProcessorStore.getState().reset());

describe("useProcessorStore", () => {
  it("replaces a radar reading every CPI so empty CPIs clear detections", () => {
    const { observe } = useProcessorStore.getState();
    observe(radar("radar", [detection(12), detection(30)]));
    expect(
      readingOf(useProcessorStore.getState().byNode.radar, "passive_radar")?.detections,
    ).toHaveLength(2);
    observe(radar("radar", []));
    expect(
      readingOf(useProcessorStore.getState().byNode.radar, "passive_radar")?.detections,
    ).toEqual([]);
  });

  it("forgets removed nodes", () => {
    const { observe, forget } = useProcessorStore.getState();
    observe(radar("a", [detection(1)]));
    observe(radar("b", [detection(2)]));
    forget(["a"]);
    expect(useProcessorStore.getState().byNode.a).toBeUndefined();
    expect(useProcessorStore.getState().byNode.b).toBeDefined();
  });

  it("keeps the same state when forgetting a node it never saw", () => {
    const before = useProcessorStore.getState().byNode;
    useProcessorStore.getState().forget(["ghost"]);
    expect(useProcessorStore.getState().byNode).toBe(before);
  });

  it("ignores events that are not processor updates", () => {
    useProcessorStore.getState().observe({ type: "Hello", data: { revision: 1, protocol: 1 } });
    expect(useProcessorStore.getState().byNode).toEqual({});
  });

  it("reads a reading only as its own type", () => {
    useProcessorStore.getState().observe(radar("radar", []));
    const state = useProcessorStore.getState().byNode.radar;
    expect(readingOf(state, "df")).toBeNull();
    expect(readingOf(undefined, "passive_radar")).toBeNull();
  });
});

describe("isStale", () => {
  it("calls a reading stale after three periods or two seconds", () => {
    expect(isStale(0, 1_999, 100)).toBe(false);
    expect(isStale(0, 2_001, 100)).toBe(true);
    expect(isStale(0, 2_000, 500)).toBe(false);
    expect(isStale(0, 2_001, 500)).toBe(true);
    expect(isStale(0, 3_000, 1_000)).toBe(false);
    expect(isStale(0, 3_001, 1_000)).toBe(true);
  });
});
