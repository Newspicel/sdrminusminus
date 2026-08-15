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
import {
  type ReactNode,
  useCallback,
  useEffect,
  useLayoutEffect,
  useMemo,
  useRef,
  useState,
} from "react";
import { Button } from "../components/BaseControls";
import { BTN_QUIET, SURFACE } from "../components/controls";
import { pushToast } from "../lib/toasts";
import type { PatchEdge, PatchGraph, PatchNode, PortRef } from "../lib/types";
import { useClipboard } from "./clipboard";
import { useWorkspaceContext } from "./context";
import {
  addEdge,
  connectionRefusal,
  edgeKey,
  edgeWarning,
  type GraphContext,
  isPinned,
  isResizable,
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

  // What a paste has just added, until the write it made comes back around and the fresh faces
  // can be selected. Only a selected face is draggable, so a paste that left the originals
  // selected would answer the gesture that always follows it by moving the wrong nodes.
  const pasted = useRef<ReadonlySet<string>>(new Set());
  const selection = nodes.filter((node) => node.selected).map((node) => node.id);
  useClipboard(
    workspace,
    // A node reached by its number key is selected in this application but not in React Flow, so
    // the keyboard's own selection is what the chord falls back to.
    selection.length > 0 ? selection : workspace.selected === null ? [] : [workspace.selected],
    useCallback((ids: readonly string[]) => {
      pasted.current = new Set(ids);
    }, []),
  );

  const held = useRef<PatchGraph>(workspace.graph);
  const context = workspace.context;
  useEffect(() => {
    if (sameGraph(held.current, workspace.graph)) {
      setEdges(toFlowEdges(workspace.graph, context));
      return;
    }
    held.current = workspace.graph;
    const fresh = pasted.current;
    const arrived = workspace.graph.nodes.some((node) => fresh.has(node.id));
    if (arrived) {
      pasted.current = new Set();
    }
    // Reconciled, not replaced: a fresh object per node would drop React Flow's own `selected`
    // flag and its measured handle bounds, so after any write the library would consider
    // nothing selected while the node still rendered as selected — and Backspace would stop
    // deleting it.
    setNodes((previous) =>
      toFlowNodes(workspace.graph).map((node) => {
        const mounted = previous.find((candidate) => candidate.id === node.id);
        const selected = arrived ? fresh.has(node.id) : (mounted?.selected ?? false);
        return mounted === undefined ? { ...node, selected } : { ...mounted, ...node, selected };
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
          // silently opt it out of every later one. A kind that cannot be resized drops any size
          // a older build stored for it, rather than being pinned to it forever.
          const natural = NODE_SIZE[node.kind];
          const { width: w, height: h } = flow;
          const resized =
            isResizable(node.kind) &&
            w != null &&
            h != null &&
            (w !== natural.w || h !== natural.h);
          const { size: _dropped, ...rest } = node;
          return {
            ...rest,
            position: { x: flow.position.x, y: flow.position.y },
            ...(resized ? { size: { w: w as number, h: h as number } } : {}),
          };
        }),
      },
    }));
  }, [workspace]);

  // React Flow owns the live geometry; `commitGeometry` runs from a microtask after a gesture
  // ends and reads it here. Written after commit so render stays pure.
  const flowRef = useRef(nodes);
  useLayoutEffect(() => {
    flowRef.current = nodes;
  });

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
  const onBeforeDelete: OnBeforeDelete<Node<FlowData>, Edge> = useCallback(
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
  const openMenu = useCallback((event: React.MouseEvent, target: Menu["target"]) => {
    event.preventDefault();
    setMenu({ x: event.clientX, y: event.clientY, target });
  }, []);

  return (
    <div className="relative flex min-h-0 flex-1 flex-col">
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
        className="min-h-0 flex-1 bg-bg"
      >
        <Background variant={BackgroundVariant.Dots} gap={24} size={1} className="!bg-bg" />
      </ReactFlow>
      {menu !== null && <ContextMenu menu={menu} onClose={() => setMenu(null)} />}
    </div>
  );
}

interface Menu {
  x: number;
  y: number;
  target: { kind: "node"; id: string } | { kind: "edge"; id: string } | { kind: "pane" };
}

/** What right-clicking offers, per thing clicked. Deliberately short: everything here is also
 * reachable from the node's own chrome or a key, and a menu that lists the whole application is
 * one nobody reads. */
function ContextMenu({ menu, onClose }: { menu: Menu; onClose: () => void }) {
  const workspace = useWorkspaceContext();
  const { fitView } = useReactFlow();
  const menuRef = useRef<HTMLDivElement>(null);
  const node = menu.target.kind === "node" ? nodeOf(workspace.graph, menu.target.id) : undefined;
  const pinned = node !== undefined && isPinned(workspace.rack, node.id);

  // A menu that outlives its context is a menu that acts on the wrong thing.
  useEffect(() => {
    const dismiss = (event: Event) => {
      if (event instanceof KeyboardEvent) {
        // Escape closes from anywhere — including a focused item, whose keydown target is
        // inside the menu and must not fall through to the pointer guard below.
        if (event.key === "Escape") {
          onClose();
        }
        return;
      }
      if (event.target instanceof Node && menuRef.current?.contains(event.target) === true) {
        return;
      }
      onClose();
    };
    window.addEventListener("keydown", dismiss);
    window.addEventListener("pointerdown", dismiss, { capture: true });
    return () => {
      window.removeEventListener("keydown", dismiss);
      window.removeEventListener("pointerdown", dismiss, { capture: true });
    };
  }, [onClose]);

  const item = (label: string, act: () => void, danger = false) => (
    <Button
      key={label}
      type="button"
      className={`${BTN_QUIET} w-full justify-start ${danger ? "hover:text-danger" : ""}`}
      onClick={() => {
        act();
        onClose();
      }}
    >
      {label}
    </Button>
  );

  const items: ReactNode[] = [];
  if (node !== undefined) {
    items.push(
      item(pinned ? "Unpin from the rack" : "Pin to the rack", () =>
        workspace.edit((snapshot) => ({
          ...snapshot,
          rack: pinned ? unpin(snapshot.rack ?? {}, node.id) : pin(snapshot.rack ?? {}, node.id),
        })),
      ),
    );
    if (node.size != null && isResizable(node.kind)) {
      items.push(
        item("Reset size", () =>
          workspace.edit((snapshot) => ({
            ...snapshot,
            graph: patchNode(snapshot.graph, node.id, ({ size: _size, ...rest }) => rest),
          })),
        ),
      );
    }
  }
  if (menu.target.kind === "edge") {
    const key = menu.target.id;
    items.push(
      item(
        "Delete wire",
        () =>
          workspace.edit((snapshot) => ({ ...snapshot, graph: removeEdge(snapshot.graph, key) })),
        true,
      ),
    );
  }
  if (menu.target.kind === "pane") {
    items.push(item("Fit the patch on screen", () => void fitView(FIT_VIEW)));
  }

  return (
    // Fixed, not absolute: the coordinates are the pointer's, and the canvas is transformed.
    <div
      ref={menuRef}
      role="menu"
      className={`${SURFACE} fixed z-40 flex w-52 flex-col p-1`}
      style={{ left: menu.x, top: menu.y }}
    >
      {items}
      {node !== undefined && (
        <span className="px-2 py-1 text-[10px] text-ink-faint">
          Backspace deletes the selection — a node or a wire.
        </span>
      )}
    </div>
  );
}

function toFlowNodes(graph: PatchGraph): Node<FlowData>[] {
  return graph.nodes.map((node) => {
    // A stored size only counts for a kind that can be resized; every other face is the size its
    // kind is (`NODE_SIZE`), whatever an older build wrote into the workspace.
    const natural = NODE_SIZE[node.kind];
    const size = (isResizable(node.kind) ? node.size : null) ?? natural;
    return {
      id: node.id,
      type: node.kind,
      position: node.position,
      data: { node },
      width: size.w,
      height: size.h ?? natural.h,
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
