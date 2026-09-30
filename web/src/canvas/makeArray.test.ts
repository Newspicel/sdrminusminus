import { describe, expect, it } from "vitest";
import type { NodeBodyOf, PatchGraph } from "../lib/types";
import { catalogBody } from "../test/catalog";
import { capabilities, deviceSet } from "../test/fixtures";
import { NODE_SIZE } from "./graph";
import {
  arrayPlacement,
  canMakeArray,
  makeArray,
  NO_SHARED_CLOCK,
  ONE_LANE,
  OPEN_FIRST,
} from "./makeArray";

const BODY = catalogBody("array") as NodeBodyOf<"array">;

function radio(): PatchGraph {
  return {
    nodes: [{ id: "kraken", position: { x: 100, y: 40 }, kind: "device", data: {} }],
    edges: [],
  };
}

const KRAKEN = deviceSet({
  capabilities: capabilities({ rx_streams: 5, coherence: "time_sync" }),
});

describe("makeArray", () => {
  it("wires every lane in order", () => {
    const graph = makeArray(radio(), "kraken", 5, BODY, "array:1");
    expect(graph.nodes.map((node) => node.kind)).toEqual(["device", "array"]);
    expect(graph.edges?.map((edge) => [edge.from.port, edge.to.port])).toEqual([
      ["iq", "lane"],
      ["iq2", "lane2"],
      ["iq3", "lane3"],
      ["iq4", "lane4"],
      ["iq5", "lane5"],
    ]);
    expect(graph.edges?.every((edge) => edge.to.node === "array:1")).toBe(true);
    const array = graph.nodes[1];
    expect(array?.kind === "array" && array.data).toEqual(BODY.data);
  });

  it("offers Make array for a multi-lane radio with a shared clock", () => {
    expect(canMakeArray(radio(), "kraken", KRAKEN)).toEqual({ ok: true, lanes: 5 });
  });

  it("refuses a one-lane radio, a radio with no shared clock, and a radio already in an array", () => {
    expect(canMakeArray(radio(), "kraken", undefined)).toEqual({ ok: false, reason: OPEN_FIRST });
    expect(canMakeArray(radio(), "kraken", deviceSet())).toEqual({ ok: false, reason: ONE_LANE });
    const loose = deviceSet({ capabilities: capabilities({ rx_streams: 2 }) });
    expect(canMakeArray(radio(), "kraken", loose)).toEqual({
      ok: false,
      reason: NO_SHARED_CLOCK,
    });
    const made = makeArray(radio(), "kraken", 5, BODY, "array:1");
    const named = {
      ...made,
      nodes: made.nodes.map((node) => (node.id === "array:1" ? { ...node, label: "North" } : node)),
    };
    expect(canMakeArray(named, "kraken", KRAKEN)).toEqual({ ok: false, reason: "In North" });
  });

  it("places the array right of the radio", () => {
    expect(arrayPlacement(radio(), "kraken")).toEqual({
      x: 100 + NODE_SIZE.device.w + 80,
      y: 40,
    });
  });
});
