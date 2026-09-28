import { afterEach, describe, expect, it } from "vitest";
import { arrayStatus, bearingRecord, radarUpdate } from "../test/fixtures";
import { useArrayStore } from "./arrays";
import { useBearingStore } from "./bearings";
import { useFusionStore } from "./fusion";
import { forgetNodes, resetNodeState } from "./nodeState";
import { useProcessorStore } from "./processors";
import { useRefusalStore } from "./refusals";
import { useSurveyStore } from "./survey";
import { goneNodes, switchedWorkspace } from "./useNodeStateSync";

function fill(node: string): void {
  useProcessorStore.getState().observe({
    type: "ProcessorUpdate",
    data: { node, reading: { type: "passive_radar", reading: radarUpdate([]) } },
  });
  useArrayStore.getState().observe({ type: "ArrayUpdate", data: { status: arrayStatus(node) } });
  useBearingStore.getState().observe({
    type: "Decoded",
    data: bearingRecord(node, {}, new Date().toISOString()),
  });
  useFusionStore.getState().set(node, { samples: 1 });
  useSurveyStore.getState().seed({
    node,
    bandwidth_hz: 12_500,
    offset_hz: 0,
    recording: false,
    cells: [],
  });
  useRefusalStore.getState().flag(node, "broken", "action");
}

const STORES = [
  () => useProcessorStore.getState().byNode,
  () => useArrayStore.getState().byNode,
  () => useBearingStore.getState().byNode,
  () => useFusionStore.getState().byNode,
  () => useSurveyStore.getState().byNode,
  () => useRefusalStore.getState().byNode,
];

afterEach(() => resetNodeState());

describe("nodeState", () => {
  it("forgetNodes clears every store for the removed ids and leaves others", () => {
    fill("gone");
    fill("kept");
    forgetNodes(["gone"]);
    for (const byNode of STORES) {
      expect(byNode().gone).toBeUndefined();
      expect(byNode().kept).toBeDefined();
    }
  });

  it("resetNodeState empties every store", () => {
    fill("a");
    resetNodeState();
    for (const byNode of STORES) {
      expect(byNode()).toEqual({});
    }
  });
});

describe("node state sync", () => {
  it("resets only when one loaded workspace gives way to another", () => {
    expect(switchedWorkspace(null, 3)).toBe(false);
    expect(switchedWorkspace(3, null)).toBe(false);
    expect(switchedWorkspace(3, 3)).toBe(false);
    expect(switchedWorkspace(3, 4)).toBe(true);
  });

  it("names the nodes that left the graph", () => {
    const kept = { id: "kept", position: { x: 0, y: 0 }, kind: "scope" } as const;
    expect(goneNodes(new Set(["gone", "kept"]), [kept])).toEqual(["gone"]);
    expect(goneNodes(new Set(), [kept])).toEqual([]);
  });
});
