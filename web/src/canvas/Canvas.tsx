// The patch view: React Flow over our own graph model (CANVAS §7). The library holds geometry
// while a gesture is in flight; the stored patch is written at the end of one, which is both
// what CANVAS §4 requires and what keeps the drag → save → `StateChanged` → refetch → re-apply
// loop from feeding itself — the same four brakes the M6 dock needed, in a smaller shape:
// writes happen only on gesture end, an incoming patch equal to the one held is not re-applied,
// and a refused wire never becomes a write at all.
import {
  Background,
  BackgroundVariant,
  type Connection,
  type Edge,
  type EdgeChange,
  type IsValidConnection,
  type Node,
  type NodeChange,
  ReactFlow,
  useEdgesState,
  useNodesState,
} from "@xyflow/react";
import { useCallback, useEffect, useRef } from "react";
import { pushToast } from "../lib/toasts";
import type { PatchEdge, PatchGraph, PatchNode } from "../lib/types";
import { useStationContext } from "./context";
import {
  addEdge,
  connectionRefusal,
  edgeKey,
  type GraphContext,
  pruneRack,
  removeEdge,
  removeNode,
  sameGraph,
} from "./graph";
import { NODE_TYPES } from "./nodes";

/** Node data React Flow carries. Only the stored node — everything live comes from context. */
export interface FlowData extends Record<string, unknown> {
  node: PatchNode;
}

export function Canvas() {
  const station = useStationContext();
  const [nodes, setNodes, onNodesChange] = useNodesState<Node<FlowData>>(
    toFlowNodes(station.graph),
  );
  const [edges, setEdges, onEdgesChange] = useEdgesState(toFlowEdges(station.graph));

  // What the canvas currently holds, as a stored patch. An incoming graph equal to this is our
  // own write echoing back through the query cache, and re-applying it would reset a drag in
  // progress; anything else is another client's patch (or the 409 recovery) and is taken.
  const held = useRef<PatchGraph>(station.graph);
  useEffect(() => {
    if (sameGraph(held.current, station.graph)) {
      return;
    }
    held.current = station.graph;
    setNodes(toFlowNodes(station.graph));
    setEdges(toFlowEdges(station.graph));
  }, [station.graph, setNodes, setEdges]);

  // Geometry is written at the end of a gesture, never per frame: one write per drag, not one
  // per pointer move (CANVAS §4).
  const commitGeometry = useCallback(() => {
    station.edit((snapshot) => ({
      ...snapshot,
      graph: {
        ...snapshot.graph,
        nodes: snapshot.graph.nodes.map((node) => {
          const flow = flowRef.current.find((candidate) => candidate.id === node.id);
          if (flow === undefined) {
            return node;
          }
          const size =
            flow.width != null && flow.height != null
              ? { size: { w: flow.width, h: flow.height } }
              : {};
          return { ...node, position: { x: flow.position.x, y: flow.position.y }, ...size };
        }),
      },
    }));
  }, [station]);

  // The live node array, read by `commitGeometry` without making it depend on every frame of a
  // drag (which would rebuild the handler mid-gesture).
  const flowRef = useRef(nodes);
  flowRef.current = nodes;

  const handleNodesChange = useCallback(
    (changes: NodeChange<Node<FlowData>>[]) => {
      onNodesChange(changes);
      for (const change of changes) {
        if (change.type === "select") {
          station.select(change.selected ? change.id : null);
        }
        // A resize reports every frame; only its last one is a gesture that ended.
        if (change.type === "dimensions" && change.resizing === false) {
          queueMicrotask(commitGeometry);
        }
        if (change.type === "remove") {
          station.edit((snapshot) => {
            const graph = removeNode(snapshot.graph, change.id);
            return { ...snapshot, graph, rack: pruneRack(snapshot.rack ?? {}, graph) };
          });
        }
      }
    },
    [onNodesChange, station, commitGeometry],
  );

  const handleEdgesChange = useCallback(
    (changes: EdgeChange<Edge>[]) => {
      onEdgesChange(changes);
      for (const change of changes) {
        if (change.type === "remove") {
          station.edit((snapshot) => ({
            ...snapshot,
            graph: removeEdge(snapshot.graph, change.id),
          }));
        }
      }
    },
    [onEdgesChange, station],
  );

  const refusal = useCallback(
    (connection: Connection | Edge): string | null => {
      const from = { node: connection.source, port: connection.sourceHandle ?? "" };
      const to = { node: connection.target, port: connection.targetHandle ?? "" };
      return connectionRefusal(station.context, station.graph, from, to);
    },
    [station],
  );

  const isValidConnection: IsValidConnection = useCallback(
    (connection) => refusal(connection) === null,
    [refusal],
  );

  const onConnect = useCallback(
    (connection: Connection) => {
      const reason = refusal(connection);
      if (reason !== null) {
        // Where the operator is looking: the drag is already refused visually, and this says
        // why in words (CANVAS §1).
        pushToast(reason);
        return;
      }
      const edge: PatchEdge = {
        from: { node: connection.source, port: connection.sourceHandle ?? "" },
        to: { node: connection.target, port: connection.targetHandle ?? "" },
      };
      station.edit((snapshot) => ({ ...snapshot, graph: addEdge(snapshot.graph, edge) }));
      // A new wire can mean a new channel to create (a channel node just given a receiver), and
      // apply is idempotent, so asking every time costs nothing.
      station.apply();
    },
    [refusal, station],
  );

  return (
    <ReactFlow
      nodes={nodes}
      edges={edges}
      nodeTypes={NODE_TYPES}
      onNodesChange={handleNodesChange}
      onEdgesChange={handleEdgesChange}
      onNodeDragStop={commitGeometry}
      onConnect={onConnect}
      isValidConnection={isValidConnection}
      onPaneClick={() => station.select(null)}
      // Desktop-only (PLAN §18): a pointer and a keyboard are assumed.
      panOnScroll
      selectionOnDrag
      minZoom={0.25}
      maxZoom={2}
      proOptions={{ hideAttribution: true }}
      className="min-h-0 flex-1 bg-bg"
    >
      <Background variant={BackgroundVariant.Dots} gap={24} size={1} className="!bg-bg" />
    </ReactFlow>
  );
}

function toFlowNodes(graph: PatchGraph): Node<FlowData>[] {
  return graph.nodes.map((node) => ({
    id: node.id,
    type: node.kind,
    position: node.position,
    data: { node },
    ...(node.size ? { width: node.size.w, height: node.size.h } : {}),
  }));
}

function toFlowEdges(graph: PatchGraph): Edge[] {
  return (graph.edges ?? []).map((edge) => ({
    id: edgeKey(edge),
    source: edge.from.node,
    sourceHandle: edge.from.port,
    target: edge.to.node,
    targetHandle: edge.to.port,
    // Hue is the data type and nothing else (CANVAS §6); the class is defined per type in CSS.
    className: `wire-${edge.from.port === "iq" ? "iq" : edge.from.port}`,
  }));
}

/** Exported for the tests that pin the mapping — the canvas is the only place the library's
 * shapes exist, and that boundary is what keeps a React Flow major off the stored patch. */
export const flowMapping = { toFlowNodes, toFlowEdges };

export type { GraphContext };
