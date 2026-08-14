import {
  Background,
  BackgroundVariant,
  type Connection,
  type Edge,
  type EdgeChange,
  type FinalConnectionState,
  type IsValidConnection,
  type Node,
  type NodeChange,
  type OnBeforeDelete,
  ReactFlow,
  useEdgesState,
  useNodesState,
  useReactFlow,
} from "@xyflow/react";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import {
  ContextMenuContent,
  ContextMenuItem,
  ContextMenuLabel,
  ContextMenuTrigger,
  ContextMenu as ShadcnContextMenu,
} from "@/components/ui/context-menu";
import { pushToast } from "../lib/toasts";
import type { PatchEdge, PatchGraph, PatchNode, PortRef } from "../lib/types";
import { useWorkspaceContext } from "./context";
import {
  addEdge,
  connectionRefusal,
  edgeKey,
  edgeWarning,
  type GraphContext,
  isPinned,
  NODE_SIZE,
  nodeOf,
  patchNode,
  pin,
  portOf,
  pruneRack,
  removeEdge,
  removeNode,
  sameGraph,
  unpin,
} from "./graph";
import { NODE_TYPES } from "./nodes";
import { closeEngineObjects } from "./remove";

/** Node data React Flow carries. Only the stored node — everything live comes from context. */
export interface FlowData extends Record<string, unknown> {
  node: PatchNode;
}

/** Framing on open. `maxZoom` keeps a one-node patch from opening magnified — a face drawn at
 * twice its size is not more legible, it is just further from what the operator will see next. */
const FIT_VIEW = { padding: 0.12, maxZoom: 1 } as const;

const DELETE_KEYS = ["Backspace", "Delete"];

export function Canvas() {
  const workspace = useWorkspaceContext();
  const [nodes, setNodes, onNodesChange] = useNodesState<Node<FlowData>>(
    toFlowNodes(workspace.graph),
  );
  const [edges, setEdges, onEdgesChange] = useEdgesState(
    toFlowEdges(workspace.graph, workspace.context),
  );

  const held = useRef<PatchGraph>(workspace.graph);
  const context = workspace.context;
  useEffect(() => {
    if (sameGraph(held.current, workspace.graph)) {
      setEdges(toFlowEdges(workspace.graph, context));
      return;
    }
    held.current = workspace.graph;
    // Reconciled, not replaced: a fresh object per node would drop React Flow's own `selected`
    // flag and its measured handle bounds, so after any write the library would consider
    // nothing selected while the node still rendered as selected — and Backspace would stop
    // deleting it.
    setNodes((previous) =>
      toFlowNodes(workspace.graph).map((node) => {
        const mounted = previous.find((candidate) => candidate.id === node.id);
        return mounted === undefined ? node : { ...mounted, ...node, selected: mounted.selected };
      }),
    );
    setEdges(toFlowEdges(workspace.graph, context));
  }, [workspace.graph, context, setNodes, setEdges]);

  const commitGeometry = useCallback(() => {
    workspace.edit((snapshot) => ({
      ...snapshot,
      graph: {
        ...snapshot.graph,
        nodes: snapshot.graph.nodes.map((node) => {
          const flow = flowRef.current.find((candidate) => candidate.id === node.id);
          if (flow === undefined) {
            return node;
          }
          // A size is stored only once the face has been resized away from what its kind opens
          // at: writing the natural size back would freeze this node at today's default and
          // silently opt it out of every later one.
          const natural = NODE_SIZE[node.kind];
          const { width: w, height: h } = flow;
          const size =
            w != null && h != null && (w !== natural.w || h !== natural.h)
              ? { size: { w, h } }
              : {};
          return { ...node, position: { x: flow.position.x, y: flow.position.y }, ...size };
        }),
      },
    }));
  }, [workspace]);

  const flowRef = useRef(nodes);
  flowRef.current = nodes;

  const handleNodesChange = useCallback(
    (changes: NodeChange<Node<FlowData>>[]) => {
      onNodesChange(changes);
      const selects = changes.filter((change) => change.type === "select");
      if (selects.length > 0) {
        workspace.select(selects.find((change) => change.selected)?.id ?? null);
      }
      for (const change of changes) {
        // A resize reports every frame; only its last one is a gesture that ended.
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

  // Backspace deletes what is selected, and a node's deletion has to close the radio or channel
  // it was driving first — the same rule the face's own ✕ follows. A refusal here cancels the
  // whole deletion, so the patch never draws a radio as gone while it is still streaming.
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

  // Only the active face is dragged by the pointer; every other one leaves the drag and the
  // wheel to the camera. React Flow stamps its own `nopan` on any node it considers draggable,
  // which is what would otherwise make the patch unscrollable wherever a face is under the
  // pointer — so "click a window before its controls answer" is also what keeps the camera free
  // (`NodeShell`'s `Active` carries the other half of the rule).
  const flowNodes = useMemo(
    () =>
      nodes.map((node) =>
        node.draggable === node.selected ? node : { ...node, draggable: node.selected },
      ),
    [nodes],
  );

  // Right-click is where an operator looks for "delete this", and a wire has nowhere else to be
  // asked: it has no chrome of its own, so without a menu the only way to cut one is to select
  // it and reach for a key nobody was told about.
  const [menu, setMenu] = useState<Menu | null>(null);
  const openMenu = useCallback((_event: React.MouseEvent, target: Menu["target"]) => {
    setMenu({ target });
  }, []);

  return (
    <ShadcnContextMenu onOpenChange={(open) => !open && setMenu(null)}>
      <ContextMenuTrigger className="relative flex min-h-0 flex-1 flex-col">
        <ReactFlow
          nodes={flowNodes}
          edges={edges}
          nodeTypes={NODE_TYPES}
          onNodesChange={handleNodesChange}
          onEdgesChange={handleEdgesChange}
          onNodeDragStop={commitGeometry}
          onConnect={onConnect}
          onConnectEnd={onConnectEnd}
          onBeforeDelete={onBeforeDelete}
          isValidConnection={isValidConnection}
          onPaneClick={() => {
            workspace.select(null);
            setMenu(null);
          }}
          onNodeClick={() => setMenu(null)}
          onNodeContextMenu={(event, node) => openMenu(event, { kind: "node", id: node.id })}
          onEdgeContextMenu={(event, edge) => openMenu(event, { kind: "edge", id: edge.id })}
          onPaneContextMenu={(event) => openMenu(event as React.MouseEvent, { kind: "pane" })}
          // Both keys, because both are what people press for "delete this".
          deleteKeyCode={DELETE_KEYS}
          panOnScroll
          panOnScrollSpeed={1}
          selectionOnDrag
          // The patch opens framed: a workspace drawn over several screens is otherwise restored at
          // whatever corner the last camera left, and the operator's first gesture is always a hunt.
          fitView
          fitViewOptions={FIT_VIEW}
          minZoom={0.15}
          maxZoom={2}
          proOptions={{ hideAttribution: true }}
          className="min-h-0 flex-1 bg-background"
        >
          <Background
            variant={BackgroundVariant.Dots}
            gap={24}
            size={1}
            className="!bg-background"
          />
        </ReactFlow>
      </ContextMenuTrigger>
      {menu !== null && <PatchContextMenu target={menu.target} />}
    </ShadcnContextMenu>
  );
}

interface Menu {
  target: { kind: "node"; id: string } | { kind: "edge"; id: string } | { kind: "pane" };
}

/** What right-clicking offers, per thing clicked. Deliberately short: everything here is also
 * reachable from the node's own chrome or a key, and a menu that lists the whole application is
 * one nobody reads. */
function PatchContextMenu({ target }: { target: Menu["target"] }) {
  const workspace = useWorkspaceContext();
  const { fitView } = useReactFlow();
  const node = target.kind === "node" ? nodeOf(workspace.graph, target.id) : undefined;
  const pinned = node !== undefined && isPinned(workspace.rack, node.id);

  return (
    <ContextMenuContent className="w-52">
      {node !== undefined && (
        <>
          <ContextMenuItem
            onClick={() =>
              workspace.edit((snapshot) => ({
                ...snapshot,
                rack: pinned
                  ? unpin(snapshot.rack ?? {}, node.id)
                  : pin(snapshot.rack ?? {}, node.id),
              }))
            }
          >
            {pinned ? "Unpin from the rack" : "Pin to the rack"}
          </ContextMenuItem>
          {node.size != null && (
            <ContextMenuItem
              onClick={() =>
                workspace.edit((snapshot) => ({
                  ...snapshot,
                  graph: patchNode(snapshot.graph, node.id, ({ size: _size, ...rest }) => rest),
                }))
              }
            >
              Reset size
            </ContextMenuItem>
          )}
        </>
      )}
      {target.kind === "edge" && (
        <ContextMenuItem
          variant="destructive"
          onClick={() =>
            workspace.edit((snapshot) => ({
              ...snapshot,
              graph: removeEdge(snapshot.graph, target.id),
            }))
          }
        >
          Delete wire
        </ContextMenuItem>
      )}
      {target.kind === "pane" && (
        <ContextMenuItem onClick={() => void fitView(FIT_VIEW)}>
          Fit the patch on screen
        </ContextMenuItem>
      )}
      {node !== undefined && (
        <ContextMenuLabel className="max-w-48 whitespace-normal text-[10px] font-normal">
          Backspace deletes the selection — a node or a wire.
        </ContextMenuLabel>
      )}
    </ContextMenuContent>
  );
}

function toFlowNodes(graph: PatchGraph): Node<FlowData>[] {
  return graph.nodes.map((node) => {
    // No stored size means the face has never been resized by hand, and it opens at the size its
    // kind needs: a width, and a height only where the content is a viewport rather than a
    // column of controls (`NODE_SIZE`). Leaving the height off is what lets React Flow measure
    // the face and give it exactly the room its instrument asks for.
    const natural = NODE_SIZE[node.kind];
    const size = node.size ?? natural;
    return {
      id: node.id,
      type: node.kind,
      position: node.position,
      data: { node },
      width: size.w,
      ...(size.h == null ? {} : { height: size.h }),
    };
  });
}

function toFlowEdges(graph: PatchGraph, context: GraphContext): Edge[] {
  return (graph.edges ?? []).map((edge) => {
    // A wire that exists but cannot carry what it says it carries is drawn as a fault on the
    // wire itself — the face at its end says what to do about it.
    const warning = edgeWarning(context, graph, edge.from, edge.to);
    const carried = portOf(context, graph, edge.from, "out")?.port_type;
    return {
      id: edgeKey(edge),
      source: edge.from.node,
      sourceHandle: edge.from.port,
      target: edge.to.node,
      targetHandle: edge.to.port,
      className: warning === null ? `wire-${carried}` : "wire-fault",
      ...(warning === null ? {} : { label: warning, labelShowBg: false }),
    };
  });
}
