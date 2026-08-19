import {
  type Connection,
  type Edge,
  type EdgeChange,
  type FinalConnectionState,
  type IsValidConnection,
  type Node,
  type NodeChange,
  type OnBeforeDelete,
} from "@xyflow/react";
import { useCallback } from "react";
import { pushToast } from "../lib/toasts";
import type { PatchEdge, PortRef } from "../lib/types";
import type { FlowData } from "./Canvas";
import type { Workspace } from "./context";
import { addEdge, connectionRefusal, pruneRack, removeEdge, removeNode } from "./graph";
import { closeEngineObjects } from "./remove";

export function useGraphChanges(
  workspace: Workspace,
  onNodesChange: (changes: NodeChange<Node<FlowData>>[]) => void,
  onEdgesChange: (changes: EdgeChange<Edge>[]) => void,
  commitGeometry: () => void,
) {
  const handleNodesChange = useCallback(
    (changes: NodeChange<Node<FlowData>>[]) => {
      onNodesChange(changes);
      const selects = changes.filter((change) => change.type === "select");
      if (selects.length > 0) {
        workspace.select(selects.find((change) => change.selected)?.id ?? null);
      }
      for (const change of changes) {
        if (change.type === "dimensions" && change.resizing === false) {
          queueMicrotask(commitGeometry);
        }
        if (change.type === "remove") {
          workspace.edit((snapshot) => {
            const graph = removeNode(snapshot.graph, change.id);
            return { ...snapshot, graph, rack: pruneRack(snapshot.rack ?? {}, graph) };
          });
        }
      }
    },
    [onNodesChange, workspace, commitGeometry],
  );

  const handleEdgesChange = useCallback(
    (changes: EdgeChange<Edge>[]) => {
      onEdgesChange(changes);
      for (const change of changes) {
        if (change.type === "remove") {
          workspace.edit((snapshot) => ({
            ...snapshot,
            graph: removeEdge(snapshot.graph, change.id),
          }));
        }
      }
    },
    [onEdgesChange, workspace],
  );

  const onBeforeDelete: OnBeforeDelete<Node<FlowData>> = useCallback(
    async ({ nodes: doomed, edges: cut }) => {
      try {
        await closeEngineObjects(
          workspace,
          doomed.map((node) => node.id),
        );
      } catch (error) {
        pushToast(error instanceof Error ? error.message : String(error));
        return false;
      }
      return { nodes: doomed, edges: cut };
    },
    [workspace],
  );

  return { handleNodesChange, handleEdgesChange, onBeforeDelete };
}

export function useConnections(workspace: Workspace) {
  const refusal = useCallback(
    (from: PortRef, to: PortRef): string | null =>
      connectionRefusal(workspace.context, workspace.graph, from, to),
    [workspace],
  );

  const isValidConnection: IsValidConnection = useCallback(
    (connection) =>
      refusal(
        { node: connection.source, port: connection.sourceHandle ?? "" },
        { node: connection.target, port: connection.targetHandle ?? "" },
      ) === null,
    [refusal],
  );

  const onConnect = useCallback(
    (connection: Connection) => {
      const edge: PatchEdge = {
        from: { node: connection.source, port: connection.sourceHandle ?? "" },
        to: { node: connection.target, port: connection.targetHandle ?? "" },
      };
      workspace.edit((snapshot) => ({ ...snapshot, graph: addEdge(snapshot.graph, edge) }));
      workspace.apply();
    },
    [workspace],
  );

  const onConnectEnd = useCallback(
    (_event: MouseEvent | TouchEvent, state: FinalConnectionState) => {
      if (state.isValid !== false || state.fromHandle == null || state.toHandle == null) {
        return;
      }
      const reason = refusal(
        { node: state.fromHandle.nodeId, port: state.fromHandle.id ?? "" },
        { node: state.toHandle.nodeId, port: state.toHandle.id ?? "" },
      );
      if (reason !== null) {
        pushToast(reason);
      }
    },
    [refusal],
  );

  return { isValidConnection, onConnect, onConnectEnd };
}
