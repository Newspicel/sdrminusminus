import { describe, expect, it } from "vitest";
import type { PatchGraph, PatchNode } from "../../lib/types";
import { followedDecoder } from "./autoFollow";

function node(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 0, y: 0 }, ...body } as PatchNode;
}

function graph(decoders: PatchNode[]): PatchGraph {
  return {
    nodes: [node("dev", { kind: "device", data: {} }), ...decoders],
    edges: decoders.map((decoder) => ({
      from: { node: "dev", port: "iq" },
      to: { node: decoder.id, port: "iq" },
    })),
  };
}

const NFM = node("nfm", { kind: "channel", data: { channel_type: "nfm" } });
const AM = node("am", { kind: "channel", data: { channel_type: "am" } });

function scope(g: PatchGraph, selected: string | null = null) {
  return { graph: g, devices: new Map(), owners: new Map(), selected };
}

describe("followedDecoder", () => {
  it("follows the only decoder on the radio", () => {
    expect(followedDecoder(scope(graph([NFM])), "dev", 0)).toBe("nfm");
  });

  it("follows the selected decoder when several share the radio", () => {
    expect(followedDecoder(scope(graph([NFM, AM])), "dev", 0)).toBeNull();
    expect(followedDecoder(scope(graph([NFM, AM]), "am"), "dev", 0)).toBe("am");
  });

  it("skips locked decoders and other lanes", () => {
    const locked = node("nfm", {
      kind: "channel",
      data: { channel_type: "nfm", tuning_locked: true },
    });
    expect(followedDecoder(scope(graph([locked, AM])), "dev", 0)).toBe("am");
    expect(followedDecoder(scope(graph([NFM])), "dev", 1)).toBeNull();
  });

  it("leaves decoders another carrier owns", () => {
    const g = graph([NFM]);
    expect(
      followedDecoder({ ...scope(g), owners: new Map([["nfm", "array"]]) }, "dev", 0),
    ).toBeNull();
  });
});
