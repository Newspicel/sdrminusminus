import { QueryClient } from "@tanstack/react-query";
import { afterEach, describe, expect, it } from "vitest";
import { arrayStatus, bearingRecord, radarUpdate } from "../test/fixtures";
import { FUSION_KEY, WORKSPACES_KEY } from "./api";
import { useArrayStore } from "./arrays";
import { useBearingStore } from "./bearings";
import { useFusionStore } from "./fusion";
import { ListenerRegistry } from "./listeners";
import { forgetNodes, resetNodeState } from "./nodeState";
import { usePositionStore } from "./position";
import { useProcessorStore } from "./processors";
import { useRefusalStore } from "./refusals";
import { surfaceHub } from "./surface";
import { useSurveyStore } from "./survey";
import type { ClientCommand } from "./types";
import { goneNodes, retryNodeState, switchedWorkspace } from "./useNodeStateSync";

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

function fixAt(node: string): void {
  usePositionStore.getState().observe({
    type: "PositionChanged",
    data: { node, fix: { latitude: 52, longitude: 13, time: "2026-09-29T12:00:00Z" } },
  });
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

  it("forgetNodes drops the fix of a removed position node but a reset keeps fixes", () => {
    fixAt("gone");
    fixAt("kept");
    forgetNodes(["gone"]);
    expect(usePositionStore.getState().sources.gone).toBeUndefined();
    expect(usePositionStore.getState().sources.kept?.fix?.latitude).toBe(52);
    resetNodeState();
    expect(usePositionStore.getState().sources.kept).toBeDefined();
    usePositionStore.getState().clear();
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

  it("asks again for node state the server could not give before the graph applied", async () => {
    const client = new QueryClient({ defaultOptions: { queries: { retry: false } } });
    let known = false;
    const fusion = async () => {
      if (!known) {
        throw new Error("No triangulation tri");
      }
      return { samples: 0 };
    };
    let workspaces = 0;
    const listing = async () => ++workspaces;
    await client.fetchQuery({ queryKey: WORKSPACES_KEY, queryFn: listing });
    await expect(
      client.fetchQuery({ queryKey: [...FUSION_KEY, "tri"], queryFn: fusion }),
    ).rejects.toThrow();
    known = true;
    retryNodeState(client);
    await expect.poll(() => client.getQueryData([...FUSION_KEY, "tri"])).toEqual({ samples: 0 });
    expect(workspaces).toBe(1);
  });

  it("asks again for a surface the server refused once the graph applies", () => {
    const registry = new ListenerRegistry();
    const sent: ClientCommand[] = [];
    surfaceHub.attach({
      send: (command) => sent.push(command),
      isConnected: () => true,
      on: (kind, listener) => registry.on(kind, listener),
    });
    const stop = surfaceHub.subscribe("tri", () => {});
    registry.emit("event", {
      type: "SurfaceRefused",
      data: { node: "tri", reason: "no_stream_ids" },
    });
    retryNodeState(new QueryClient());
    stop();
    surfaceHub.detach();
    expect(sent.filter((command) => command.type === "SubscribeSurface")).toHaveLength(2);
  });

  it("names the nodes that left the graph", () => {
    const kept = { id: "kept", position: { x: 0, y: 0 }, kind: "scope" } as const;
    expect(goneNodes(new Set(["gone", "kept"]), [kept])).toEqual(["gone"]);
    expect(goneNodes(new Set(), [kept])).toEqual([]);
  });
});
