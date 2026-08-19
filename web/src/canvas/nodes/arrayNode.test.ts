import { describe, expect, it } from "vitest";
import type { PatchGraph } from "../../lib/types";
import { addEdge, removeEdge, settleArrays } from "../graph";
import { arrayHolding, arrayMembers } from "./arrayNode";

function graph(count: number, wires: [string, string][]): PatchGraph {
  return {
    nodes: [
      {
        id: "one",
        kind: "device",
        data: { device: { backend: "rtlsdr", key: "0001" } },
        position: { x: 0, y: 0 },
      },
      {
        id: "two",
        kind: "device",
        data: { device: { backend: "rtlsdr", key: "0002" } },
        position: { x: 0, y: 0 },
      },
      {
        id: "bench",
        kind: "array",
        data: { members: count, coherence: "time_sync", shared_tuning: true },
        position: { x: 0, y: 0 },
      },
    ],
    edges: wires.map(([from, port]) => ({
      from: { node: from, port: "iq" },
      to: { node: "bench", port },
    })),
  };
}

describe("arrayMembers", () => {
  it("reads the radios off the wires, in the order they arrive", () => {
    const found = arrayMembers(
      graph(2, [
        ["two", "iq2"],
        ["one", "iq"],
      ]),
      "bench",
    );
    expect(found.map((member) => member.node)).toEqual(["one", "two"]);
    expect(found[0]?.device?.key).toBe("0001");
  });

  it("has nothing to say about a node that is not an array", () => {
    expect(arrayMembers(graph(1, [["one", "iq"]]), "one")).toEqual([]);
  });
});

describe("arrayHolding", () => {
  it("names the array that has taken a radio", () => {
    const wired = graph(1, [["one", "iq"]]);
    expect(arrayHolding(wired, "one")).toBe("bench");
    expect(arrayHolding(wired, "two")).toBeNull();
  });
});

/// What the node itself says it carries, which is what the ports are drawn from.
function members(patch: PatchGraph): number {
  const found = patch.nodes.find((candidate) => candidate.id === "bench");
  return found?.kind === "array" ? found.data.members : -1;
}

describe("settleArrays", () => {
  it("grows a free input as each radio is wired in", () => {
    const empty = graph(0, []);
    const one = addEdge(empty, {
      from: { node: "one", port: "iq" },
      to: { node: "bench", port: "iq" },
    });
    expect(members(one)).toBe(1);
    const two = addEdge(one, {
      from: { node: "two", port: "iq" },
      to: { node: "bench", port: "iq2" },
    });
    expect(members(two)).toBe(2);
  });

  it("takes the port away again when the radio is unwired", () => {
    const wired = graph(2, [
      ["one", "iq"],
      ["two", "iq2"],
    ]);
    const cut = removeEdge(wired, "two.iq->bench.iq2");
    expect(members(cut)).toBe(1);
  });

  it("leaves a graph with no arrays exactly as it was", () => {
    const plain: PatchGraph = { nodes: [], edges: [] };
    expect(settleArrays(plain)).toBe(plain);
  });
});
