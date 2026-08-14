import type { WorkspaceSnapshot } from "../lib/types";
import { isPinned, patchNode, pin, removeEdge, unpin } from "./graph";

export type PatchMenuAction =
  | { kind: "toggle-pin"; node: string }
  | { kind: "reset-size"; node: string }
  | { kind: "delete-edge"; edge: string }
  | { kind: "fit" };

interface PatchMenuEffects {
  edit: (edit: (snapshot: WorkspaceSnapshot) => WorkspaceSnapshot) => void;
  fit: () => void;
  close: () => void;
}

export function runPatchMenuAction(action: PatchMenuAction, effects: PatchMenuEffects): void {
  if (action.kind === "fit") {
    effects.fit();
  } else {
    effects.edit((snapshot) => {
      if (action.kind === "toggle-pin") {
        const rack = snapshot.rack ?? {};
        return {
          ...snapshot,
          rack: isPinned(rack, action.node) ? unpin(rack, action.node) : pin(rack, action.node),
        };
      }
      if (action.kind === "reset-size") {
        return {
          ...snapshot,
          graph: patchNode(snapshot.graph, action.node, ({ size: _size, ...node }) => node),
        };
      }
      return { ...snapshot, graph: removeEdge(snapshot.graph, action.edge) };
    });
  }
  effects.close();
}
