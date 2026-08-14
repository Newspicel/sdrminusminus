import { describe, expect, it, vi } from "vitest";
import type { WorkspaceSnapshot } from "../lib/types";
import { type PatchMenuAction, runPatchMenuAction } from "./contextMenu";
import { edgeKey, isPinned } from "./graph";

function snapshot(): WorkspaceSnapshot {
  return {
    version: 1,
    graph: {
      nodes: [
        {
          id: "device",
          kind: "device",
          position: { x: 0, y: 0 },
          size: { w: 320, h: 240 },
          data: {},
        },
        { id: "scope", kind: "scope", position: { x: 400, y: 0 } },
      ],
      edges: [
        {
          from: { node: "device", port: "iq" },
          to: { node: "scope", port: "iq" },
        },
      ],
    },
    rack: { slots: [] },
  };
}

function harness(initial = snapshot()) {
  let current = initial;
  const fit = vi.fn();
  const close = vi.fn();
  const run = (action: PatchMenuAction): void =>
    runPatchMenuAction(action, {
      edit: (edit) => {
        current = edit(current);
      },
      fit,
      close,
    });
  return { current: () => current, fit, close, run };
}

describe("patch context menu actions", () => {
  it("pins and unpins a node, dismissing after each action", () => {
    const menu = harness();
    menu.run({ kind: "toggle-pin", node: "scope" });
    expect(isPinned(menu.current().rack ?? {}, "scope")).toBe(true);

    menu.run({ kind: "toggle-pin", node: "scope" });
    expect(isPinned(menu.current().rack ?? {}, "scope")).toBe(false);
    expect(menu.close).toHaveBeenCalledTimes(2);
  });

  it("resets a node size and deletes a wire", () => {
    const menu = harness();
    menu.run({ kind: "reset-size", node: "device" });
    expect(menu.current().graph.nodes[0]).not.toHaveProperty("size");

    const edge = menu.current().graph.edges?.[0];
    expect(edge).toBeDefined();
    menu.run({ kind: "delete-edge", edge: edgeKey(edge!) });
    expect(menu.current().graph.edges).toEqual([]);
    expect(menu.close).toHaveBeenCalledTimes(2);
  });

  it("fits the pane and dismisses the menu", () => {
    const menu = harness();
    menu.run({ kind: "fit" });
    expect(menu.fit).toHaveBeenCalledOnce();
    expect(menu.close).toHaveBeenCalledOnce();
  });
});
