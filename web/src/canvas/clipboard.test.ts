import { describe, expect, it } from "vitest";
import type { PatchGraph, PatchNode } from "../lib/types";
import { copyNodes, pasteNodes, pasteRefusal } from "./clipboard";
import { MAX_EDGES, MAX_NODES } from "./graph";

function node(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 100, y: 200 }, ...body } as PatchNode;
}

const graph: PatchGraph = {
  nodes: [
    node("dev", { kind: "device", data: { device: { backend: "rtlsdr", serial: "7" } } }),
    node("ch", { kind: "channel", data: { channel_type: "nfm" }, label: "Tower" }),
    node("spk", { kind: "speaker" }),
  ],
  edges: [
    { from: { node: "dev", port: "iq2" }, to: { node: "ch", port: "iq" } },
    { from: { node: "ch", port: "audio" }, to: { node: "spk", port: "audio" } },
  ],
};

describe("copyNodes", () => {
  it("takes the named nodes and only the wires with both ends among them", () => {
    const copied = copyNodes(graph, ["ch", "spk"]);

    expect(copied?.nodes.map((entry) => entry.id)).toEqual(["ch", "spk"]);
    expect(copied?.edges).toEqual([
      { from: { node: "ch", port: "audio" }, to: { node: "spk", port: "audio" } },
    ]);
  });

  it("has nothing to say about an empty selection", () => {
    expect(copyNodes(graph, [])).toBeNull();
    expect(copyNodes(graph, ["gone"])).toBeNull();
  });
});

describe("pasteNodes", () => {
  it("adds the copy under fresh ids, offset, with its wires rewritten to them", () => {
    const copied = copyNodes(graph, ["ch", "spk"]);
    if (copied === null) {
      throw new Error("nothing copied");
    }
    const pasted = pasteNodes(graph, copied, { x: 32, y: 32 }, ["ch2", "spk2"]);

    expect(pasted.nodes.map((entry) => entry.id)).toEqual(["dev", "ch", "spk", "ch2", "spk2"]);
    expect(pasted.nodes[3]).toEqual({
      id: "ch2",
      kind: "channel",
      data: { channel_type: "nfm" },
      label: "Tower",
      position: { x: 132, y: 232 },
    });
    expect((pasted.edges ?? []).slice(2)).toEqual([
      { from: { node: "ch2", port: "audio" }, to: { node: "spk2", port: "audio" } },
    ]);
    expect(graph.nodes).toHaveLength(3);
  });

  it("leaves the copy of a device naming no radio", () => {
    const copied = copyNodes(graph, ["dev"]);
    if (copied === null) {
      throw new Error("nothing copied");
    }
    const pasted = pasteNodes(graph, copied, { x: 32, y: 32 }, ["dev2"]);

    expect(pasted.nodes[3]).toEqual({
      id: "dev2",
      kind: "device",
      data: {},
      position: { x: 132, y: 232 },
    });
  });

  it("keeps a wire that ran between two copied nodes off the originals", () => {
    const copied = copyNodes(graph, ["dev", "ch"]);
    if (copied === null) {
      throw new Error("nothing copied");
    }
    const pasted = pasteNodes(graph, copied, { x: 64, y: 64 }, ["dev2", "ch2"]);

    expect(pasted.edges).toContainEqual({
      from: { node: "dev2", port: "iq2" },
      to: { node: "ch2", port: "iq" },
    });
    expect((pasted.edges ?? []).filter((edge) => edge.to.node === "ch")).toHaveLength(1);
  });
});

describe("pasteRefusal", () => {
  it("accepts a paste that fits", () => {
    const copied = copyNodes(graph, ["spk"]);
    expect(copied === null ? "unreachable" : pasteRefusal(graph, copied)).toBeNull();
  });

  it("refuses one that would cross what the server stores", () => {
    const many = {
      nodes: Array.from({ length: MAX_NODES }, (_, index) =>
        node(`n${index}`, { kind: "speaker" }),
      ),
      edges: [],
    };
    const copied = copyNodes(graph, ["spk"]);
    if (copied === null) {
      throw new Error("nothing copied");
    }
    expect(pasteRefusal(many, copied)).toBe(`a patch holds ${MAX_NODES} nodes`);

    const wired = {
      nodes: graph.nodes,
      edges: Array.from({ length: MAX_EDGES }, () => ({
        from: { node: "ch", port: "audio" },
        to: { node: "spk", port: "audio" },
      })),
    };
    const pair = copyNodes(graph, ["ch", "spk"]);
    if (pair === null) {
      throw new Error("nothing copied");
    }
    expect(pasteRefusal(wired, pair)).toBe(`a patch holds ${MAX_EDGES} wires`);
  });
});
