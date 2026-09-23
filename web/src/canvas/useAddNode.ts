import type { NodeKind, PatchNode, Position } from "../lib/types";
import { useWorkspaceContext } from "./context";
import { addNode, newNodeId, nodeIds } from "./graph";
import { newNodeBody, startsOnItsOwn } from "./newNode";
import { useNodePlacement } from "./placement";

export type AddNode = (kind: NodeKind, channelType?: string, at?: Position) => void;

export function useAddNode(): AddNode {
  const workspace = useWorkspaceContext();
  const placeNode = useNodePlacement();

  return (kind, channelType, at) => {
    const id = newNodeId(kind, nodeIds(workspace.graph));
    workspace.edit((snapshot) => {
      const node = {
        id,
        position: at ?? placeNode(snapshot.graph, kind),
        ...newNodeBody(kind, { channelType }),
      } as PatchNode;
      return { ...snapshot, graph: addNode(snapshot.graph, node) };
    });
    workspace.select(id);
    if (startsOnItsOwn(kind)) {
      workspace.apply();
    }
  };
}
