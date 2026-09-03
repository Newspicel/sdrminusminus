import { describe, expect, it } from "vitest";
import type { PatchNode } from "../lib/types";
import { applyToasts } from "./applyToasts";

function node(id: string, body: Partial<PatchNode> & Pick<PatchNode, "kind">): PatchNode {
  return { id, position: { x: 0, y: 0 }, ...body } as PatchNode;
}

const nodes: PatchNode[] = [
  node("device:a3ca5d1f", { kind: "device", label: "RTL-SDR 0", data: {} }),
  node("device:bare", { kind: "device", data: {} }),
  node("channel:1", { kind: "channel", data: { channel_type: "nfm" } }),
];

describe("applyToasts", () => {
  it("has nothing to say without a report", () => {
    expect(applyToasts(null, nodes)).toEqual([]);
    expect(applyToasts({ bound: [], created: 0, opened: 0 }, nodes)).toEqual([]);
  });

  it("names a node by its label, a channel by its type and falls back to the id", () => {
    expect(
      applyToasts(
        {
          bound: [],
          created: 0,
          opened: 0,
          absent: ["device:a3ca5d1f", "device:bare"],
          refused: [{ node: "channel:1", reason: "no room left" }],
        },
        nodes,
      ),
    ).toEqual([
      "NFM: no room left",
      "RTL-SDR 0: its radio is not connected, so nothing on it was started",
      "device:bare: its radio is not connected, so nothing on it was started",
    ]);
  });
});
